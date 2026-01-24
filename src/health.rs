use std::{
    fs::{read_to_string, write},
    path::PathBuf,
    time::SystemTime,
};

use crate::{
    FailureType, GenResult, create_path,
    errors::SignInFailure,
    get_data,
    webcom::ical::{CALENDAR_VERSION, get_ical_path, load_ical_file},
    webcom::shift::Shift,
};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use tracing::*;
use url::Url;

#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct ApplicationLogbook {
    pub state: FailureType,
    // How long the application has been in the same state
    pub repeat_count: u64,
    #[serde(default)]
    pub execution_timestamp: NaiveDateTime,
    pub application_state: ApplicationState,
}

impl ApplicationLogbook {
    pub fn get_naive_datetime() -> NaiveDateTime {
        chrono::offset::Utc::now().naive_utc()
    }

    // Load the previous logbook, or create a new one if that fails
    pub fn load() -> ApplicationLogbook {
        let path = ApplicationLogbook::create_path();
        // This match statement tries to load the previous logbook, otherwise it creates a new one
        let mut logbook = match || -> GenResult<Self> {
            let logbook_string = read_to_string(path)?;
            Ok(serde_json::from_str(&logbook_string)?)
        }() {
            Ok(logbook) => logbook,
            Err(err) => {
                info!(
                    "Loading previous logbook failed, err: {}. Creating new one",
                    err.to_string()
                );
                ApplicationLogbook::default()
            }
        };
        let current_system_time = SystemTime::now();
        logbook.application_state.system_time = Some(current_system_time);
        logbook
    }

    pub fn generate_shift_statistics(&mut self, shifts: &Vec<Shift>, non_relevant_shifts: usize) {
        let number_of_shifts = shifts.len() as u64;
        let number_of_broken_shifts = shifts.iter().filter(|shift| shift.is_broken).count() as u64;
        let number_of_failed_broken_shifts = shifts
            .iter()
            .filter(|shift| shift.is_broken && shift.broken_period.is_none())
            .count() as u64;
        self.application_state.broken_shifts = number_of_broken_shifts;
        self.application_state.shifts = number_of_shifts;
        self.application_state.non_relevant_shifts = non_relevant_shifts as u64;
        self.application_state.failed_broken_shifts = number_of_failed_broken_shifts;
    }

    pub fn add_failed_shifts(&mut self, number: u64, replace: bool) {
        match replace {
            true => self.application_state.failed_shifts = number,
            false => self.application_state.failed_shifts += number,
        }
    }

    // Populate the logbook values and save it to disk
    pub fn save(&mut self, state: &FailureType) -> GenResult<()> {
        let path = ApplicationLogbook::create_path();
        let execution_time = SystemTime::now()
            .duration_since(
                self.application_state
                    .system_time
                    .ok_or("Previous system time not set!")?,
            )
            .and_then(|duration| Ok(duration.as_millis() as u64))
            .unwrap_or_default();
        self.application_state.execution_time_ms = execution_time;
        self.execution_timestamp = Self::get_naive_datetime();
        self.repeat_count = if self.state == *state {
            self.repeat_count + 1
        } else {
            0
        };
        self.application_state.calendar_version = CALENDAR_VERSION.to_owned();
        self.state = state.clone();
        write(path, serde_json::to_string_pretty(&self)?)?;
        Ok(())
    }

    fn create_path() -> PathBuf {
        create_path("logbook.json")
    }
}

#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct ApplicationState {
    #[serde(default, skip)]
    system_time: Option<SystemTime>,
    pub execution_time_ms: u64,
    pub shifts: u64,
    pub broken_shifts: u64,
    pub non_relevant_shifts: u64,
    pub failed_shifts: u64,
    pub failed_broken_shifts: u64,
    pub calendar_version: String,
}

pub async fn send_heartbeat(reason: &FailureType) -> GenResult<()> {
    if reason == &FailureType::TriesExceeded {
        debug!("Not sending heartbeat due to tries exceeded");
        return Ok(());
    }

    let (user, properties) = get_data();
    let personeelsnummer = &user.user_name;
    let mut request_url: Url = properties.kuma_properties.domain.clone().parse()?;
    request_url.set_path(&format!("/api/push/{personeelsnummer}"));
    request_url.set_query(Some(&format!(
        "status={}&msg={}&ping=",
        match reason.clone() {
            FailureType::GeckoEngine => "down",
            FailureType::SignInFailed(failure)
                if matches!(
                    failure,
                    SignInFailure::WebcomDown
                        | SignInFailure::TooManyTries
                        | SignInFailure::Other(_)
                ) =>
                "down",
            _ => "up",
        },
        reason.to_string()
    )));
    reqwest::get(request_url).await?;
    Ok(())
}

pub fn update_calendar_exit_code(
    previous_exit_code: &FailureType,
    current_exit_code: &FailureType,
) -> GenResult<()> {
    let ical_path = get_ical_path();
    let calendar = load_ical_file(&ical_path)?.to_string();
    let formatted_previous_exit_code =
        serde_json::to_string(&previous_exit_code).unwrap_or("OK".to_owned());
    let formatted_current_exit_code =
        serde_json::to_string(&current_exit_code).unwrap_or("OK".to_owned());
    let calendar = calendar.replace(
        &format!("X-EXIT-CODE:{formatted_previous_exit_code}"),
        &format!("X-EXIT-CODE:{formatted_current_exit_code}"),
    );
    write(ical_path, calendar.to_string().as_bytes())?;
    Ok(())
}
