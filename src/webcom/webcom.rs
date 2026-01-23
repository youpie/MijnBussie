use std::path::PathBuf;
use std::sync::Arc;

use crate::errors::ResultLog;
use crate::webcom::gebroken_shifts;
use crate::webcom::shift::Shift;
use crate::{
    FALLBACK_URL, GenError, GenResult, MAIN_URL, create_path,
    errors::{FailureType, IncorrectCredentialsCount},
    execution::timer::StartRequest,
    get_data, get_set_name,
    health::{ApplicationLogbook, send_heartbeat, update_calendar_exit_code},
    webcom::{
        email::{self, send_errors, send_welcome_mail},
        ical::{
            self, NON_RELEVANT_EVENTS_PATH, RELEVANT_EVENTS_PATH, create_ical, get_ical_path,
            get_previous_shifts, split_relevant_shifts,
        },
        parsing::{
            load_calendar, load_current_month_shifts, load_next_month_shifts,
            load_previous_month_shifts,
        },
        webdriver::{get_driver, wait_until_loaded, wait_untill_redirect},
    },
};
use dotenvy::var;
use thirtyfour::WebDriver;
use tokio::fs::{self, write};
use tokio::sync::mpsc::Sender;
use tracing::*;

async fn init_shifts(driver: &WebDriver) -> GenResult<(Vec<Shift>, Vec<Shift>)> {
    info!(
        "Existing calendar file not found, adding two extra months of shifts and removing partial calendars"
    );
    _ = fs::remove_file(PathBuf::from(NON_RELEVANT_EVENTS_PATH)).await;
    _ = fs::remove_file(PathBuf::from(RELEVANT_EVENTS_PATH)).await;
    let found_shifts = load_previous_month_shifts(&driver, 2).await?;
    debug!("Found a total of {} shifts", found_shifts.len());
    Ok(split_relevant_shifts(found_shifts))
}

// Main program logic that has to run, if it fails it will all be reran.
async fn main_program(
    driver: &WebDriver,
    retry_count: usize,
    logbook: &mut ApplicationLogbook,
) -> GenResult<()> {
    let (user, _properties) = get_data();
    let personeelsnummer = user.personeelsnummer.clone();
    let password = user.password.clone();
    driver.delete_all_cookies().await?;
    info!("Loading site: {}..", MAIN_URL);
    match driver.goto(MAIN_URL).await {
        Ok(_) => wait_untill_redirect(&driver).await?,
        Err(_) => {
            error!(
                "Failed waiting for redirect. Going to fallback {}",
                FALLBACK_URL[retry_count % FALLBACK_URL.len()]
            );
            driver
                .goto(FALLBACK_URL[retry_count % FALLBACK_URL.len()])
                .await
                .map_err(|_| Box::new(FailureType::ConnectError))?
        }
    };
    load_calendar(&driver, personeelsnummer, password).await?;
    wait_until_loaded(&driver).await?;

    let mut new_shifts = load_current_month_shifts(&driver, logbook).await?;
    let mut non_relevant_shifts = vec![];
    let ical_path = get_ical_path();
    if !ical_path.exists() {
        let mut initial_shifts = init_shifts(driver).await?;
        new_shifts.append(&mut initial_shifts.0);
        non_relevant_shifts.append(&mut initial_shifts.1);
        debug!(
            "Got {} relevant and {} non-relevant events",
            new_shifts.len(),
            non_relevant_shifts.len()
        );
    } else {
        debug!("Existing calendar file found");
        new_shifts.append(&mut load_previous_month_shifts(&driver, 0).await?);
    }
    new_shifts.append(&mut load_next_month_shifts(&driver, logbook).await?);
    info!("Found {} shifts", new_shifts.len());
    // If getting previous shift information failed, just create an empty one. Because it will cause a new calendar to be created
    let mut previous_shifts = get_previous_shifts()
        .warn_owned("Getting previous shift information")
        .ok()
        .flatten()
        .unwrap_or_default();
    non_relevant_shifts.append(&mut previous_shifts.non_relevant_shifts);
    let previous_relevant_shifts = previous_shifts.relevant_shifts;

    // The main send email function will return the broken shifts that are new or have changed.
    // This is because the send email functions uses the previous shifts and scans for new shifts
    let mut relevant_shifts = match email::send_emails(new_shifts, previous_relevant_shifts) {
        Ok(shifts) => shifts,
        Err(err) => return Err(err),
    };

    let non_relevant_shift_len = non_relevant_shifts.len();
    relevant_shifts.append(&mut non_relevant_shifts);
    let broken_shifts;
    if var("SKIP_BROKEN").unwrap_or("false".to_owned()) != "true" {
        relevant_shifts =
            gebroken_shifts::load_broken_shift_information(&driver, &relevant_shifts).await?; // Replace the shifts with the newly created list of broken shifts
        ical::save_partial_shift_files(&relevant_shifts).error("Saving partial shift files");
        broken_shifts = gebroken_shifts::split_broken_shifts(&relevant_shifts);
    } else {
        broken_shifts = relevant_shifts.clone();
    }

    let midnight_stopped_shifts = gebroken_shifts::stop_shift_at_midnight(&broken_shifts)?;
    let mut night_split_shifts = gebroken_shifts::split_night_shift(&midnight_stopped_shifts)?;
    night_split_shifts.sort_by_key(|shift| shift.magic_number);
    night_split_shifts.dedup();
    debug!("Saving {} shifts", night_split_shifts.len());
    let calendar = create_ical(&night_split_shifts, &relevant_shifts, &logbook.state)?;
    send_welcome_mail(&ical_path, false)?;
    info!("Writing to: {:?}", &ical_path);
    write(ical_path, calendar.as_bytes()).await?;
    logbook.generate_shift_statistics(&relevant_shifts, non_relevant_shift_len);
    Ok(())
}

// Create file on disk to show webcom ical is currently active
// Always delete the file at the beginning of this function
// Only create a new file if start reason is Some
async fn create_delete_lock(start_reason: Option<&StartRequest>) -> GenResult<()> {
    let path = create_path("active");
    if path.exists() {
        debug!("Removing existing lock file");
        fs::remove_file(&path).await?;
    }
    if let Some(start_reason) = start_reason {
        debug!("Creating new lock file");
        let text = serde_json::to_string(start_reason).unwrap_or_default();
        write(&path, text.as_bytes()).await?;
    }
    Ok(())
}

#[derive(PartialEq)]
pub enum ResumeReason {
    Ok,
    NewPassword,

    // Do not resume on these ones
    IncorrectCredentials,
    SigninFailureReduce,
}

pub async fn webcom_instance(
    start_reason: StartRequest,
    sender: Arc<Sender<StartRequest>>,
) -> FailureType {
    let (_user, properties) = get_data();

    create_delete_lock(Some(&start_reason))
        .await
        .warn("Creating Lock file");

    let name = get_set_name(None);
    let mut logbook = ApplicationLogbook::load();
    let mut failure_counter = IncorrectCredentialsCount::load();

    let mut current_exit_code = FailureType::default();
    let previous_exit_code = logbook.clone().state;
    let mut running_errors: Vec<GenError> = vec![];

    let mut allow_execution = true;
    let mut retry_count: usize = 0;
    let max_retry_count: usize = properties.execution_retry_count as usize;

    // Check if the program is allowed to run, or not due to failed sign-in
    let resume_reason: ResumeReason = failure_counter.sign_in_failed_check();
    if start_reason != StartRequest::Force {
        if matches!(
            resume_reason,
            ResumeReason::IncorrectCredentials | ResumeReason::SigninFailureReduce
        ) {
            // If there is a reason to not resume, it is a sign in failure reason, so you can safely assume the failure counter error is set
            current_exit_code =
                FailureType::SignInFailed(failure_counter.error.clone().unwrap_or_default());
            clean_execution(&mut logbook, &current_exit_code, sender).await;

            return current_exit_code;
        }
    } else {
        info!("Force resuming execution");
    }

    // Load the driver, do an early return if it fails
    let driver = match get_driver(&mut logbook).await {
        Ok(driver) => driver,
        Err(err) => {
            error!("Failed to get driver! error: {}", err.to_string());
            current_exit_code = FailureType::GeckoEngine;
            clean_execution(&mut logbook, &current_exit_code, sender).await;
            return current_exit_code;
        }
    };

    while retry_count < max_retry_count && allow_execution {
        match main_program(&driver, retry_count, &mut logbook)
            .await
            .warn_owned("Main Program")
        {
            Ok(()) => {
                failure_counter
                    .update_signin_failure(false, &resume_reason, None)
                    .warn("Updating signin failure");
                allow_execution = false;
            }
            Err(err) if err.downcast_ref::<FailureType>().is_some() => {
                let webcom_error = err
                    .downcast_ref::<FailureType>()
                    .cloned()
                    .unwrap_or_default();
                match webcom_error.clone() {
                    FailureType::SignInFailed(signin_failure) => {
                        allow_execution = false;
                        failure_counter
                            .update_signin_failure(
                                true,
                                &resume_reason,
                                Some(signin_failure.clone()),
                            )
                            .warn("Updating signin failure 2");
                        current_exit_code = webcom_error;
                    }
                    FailureType::ConnectError => {
                        allow_execution = false;
                        current_exit_code = FailureType::ConnectError;
                    }
                    _ => {
                        running_errors.push(err);
                    }
                }
            }
            Err(err) => {
                running_errors.push(err);
            }
        };
        retry_count += 1;
    }

    if running_errors.is_empty() {
        info!("Alles is in een keer goed gegaan, jippie!");
    } else if running_errors.len() < max_retry_count {
        warn!("Errors have occured, but succeded in the end");
    } else {
        current_exit_code = FailureType::TriesExceeded;
        send_errors(&running_errors, &name).warn("Sending errors in loop");
    }

    _ = driver.quit().await.is_err_and(|_| {
        current_exit_code = FailureType::GeckoEngine;
        true
    });

    // Update the exit code in the calendar if it is not equal to the previous value
    if previous_exit_code != current_exit_code {
        warn!("Previous exit code was different than current, need to update");
        update_calendar_exit_code(&previous_exit_code, &current_exit_code)
            .warn("Updating calendar exit code");
    }

    clean_execution(&mut logbook, &current_exit_code, sender).await;

    current_exit_code
}

async fn clean_execution(
    logbook: &mut ApplicationLogbook,
    exit_code: &FailureType,
    sender: Arc<Sender<StartRequest>>,
) {
    logbook.save(exit_code).warn("Saving logbook in loop");
    create_delete_lock(None).await.warn("Removing lock");
    sender
        .try_send(StartRequest::ExecutionFinished(exit_code.clone()))
        .warn("Sending exit code back to instance manager");
    send_heartbeat(&exit_code)
        .await
        .warn("Sending Heartbeat in loop");
}
