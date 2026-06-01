use anyhow::Context;
use dotenvy::var;
use std::path::PathBuf;
use std::sync::Arc;
use thirtyfour::WebDriver;
use tokio::fs::{self, write};
use tokio::sync::mpsc::Sender;
use tracing_futures::WithSubscriber;

use crate::health::{send_heartbeat, update_calendar_exit_code};
use crate::instance::deletion::InstanceStanding;
use crate::{FALLBACK_URL, MAIN_URL};

use super::*;

async fn init_shifts(driver: &WebDriver) -> GenResult<(Vec<Shift>, Vec<Shift>)> {
    info!(
        "Existing calendar file not found, adding two extra months of shifts and removing partial calendars"
    );
    _ = fs::remove_file(PathBuf::from(ical::NON_RELEVANT_EVENTS_PATH)).await;
    _ = fs::remove_file(PathBuf::from(ical::RELEVANT_EVENTS_PATH)).await;
    let found_shifts = parsing::load_previous_month_shifts(&driver, 2).await?;
    debug!("Found a total of {} shifts", found_shifts.len());
    Ok(ical::split_relevant_shifts(found_shifts))
}

/// If Webcom is running
/// Return false
/// if it is not
/// get the exit code of the previous join handle and set it
/// spawn a new webcom instance
pub async fn spawn_webcom_instance(
    start_request: &StartRequest,
    exit_code_sender: Arc<Sender<StartRequest>>,
    thread_store: &mut Option<(JoinHandle<FailureType>, bool)>,
    last_exit_code: &mut FailureType,
) -> Started {
    if let Some(thread) = thread_store
        && !thread.0.is_finished()
    {
        return Started::AlreadyActive;
    } else if let Some((thread, _)) = thread_store {
        *last_exit_code = thread.await.unwrap_or_default();
    }
    let (user, properties) = get_data();
    *thread_store = Some((
        tokio::spawn(
            USER_PROPERTIES
                .scope(
                    RefCell::new(Some(user)),
                    GENERAL_PROPERTIES.scope(
                        RefCell::new(Some(properties)),
                        webcom_instance(start_request.clone(), exit_code_sender),
                    ),
                )
                .with_current_subscriber(),
        ),
        false,
    ));
    Started::Started
}

pub fn is_webcom_instance_active(
    thread_store: &Option<(JoinHandle<FailureType>, bool)>,
) -> ActiveState {
    let (user, _data) = get_data();
    let standing = InstanceStanding::get_standing();
    if let Some(state) = thread_store
        && !state.0.is_finished()
    {
        if state.1 {
            ActiveState::SignedIn
        } else {
            ActiveState::Active
        }
    } else if standing == InstanceStanding::Fresh && user.online_created {
        ActiveState::Dirty
    } else {
        ActiveState::Dead
    }
}

// Main program logic that has to run, if it fails it will all be reran.
async fn main_program(
    driver: &WebDriver,
    retry_count: usize,
    logbook: &mut ApplicationLogbook,
    sender: Arc<Sender<StartRequest>>,
) -> GenResult<Option<FailureType>> {
    let mut non_critical_error = None;
    let (user, _properties) = get_data();
    let personeelsnummer = user.personeelsnummer.clone();
    let password = user.password.clone();
    driver.delete_all_cookies().await?;
    info!("Loading site: {}..", MAIN_URL);
    match driver.goto(MAIN_URL).await {
        Ok(_) => webdriver::wait_untill_redirect(&driver).await?,
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
    parsing::sign_in_and_open_calendar_view(&driver, personeelsnummer, password)
        .await
        .context("Signing in")?;
    // After the last function, signing in was succesful. So return that to the instance
    sender
        .send(StartRequest::SignedIn)
        .await
        .warn("Informing sign in");
    webdriver::wait_until_loaded(&driver)
        .await
        .context("Waiting until page loaded")?;
    let mut send_welcome = false;
    let mut new_shifts = parsing::load_current_month_shifts(&driver, logbook)
        .await
        .context("Loading current shifts")?;
    let mut non_relevant_shifts = vec![];
    let ical_path = ical::get_ical_path();
    if !ical_path.exists() {
        send_welcome = true;
        let mut initial_shifts = init_shifts(driver).await.context("Initializing shifts")?;
        new_shifts.append(&mut initial_shifts.0);
        non_relevant_shifts.append(&mut initial_shifts.1);
        debug!(
            "Got {} relevant and {} non-relevant events",
            new_shifts.len(),
            non_relevant_shifts.len()
        );
    } else {
        debug!("Existing calendar file found");
        new_shifts.append(
            &mut parsing::load_previous_month_shifts(&driver, 0)
                .await
                .context("Loading previous months shifts")?,
        );
    }
    new_shifts.append(
        &mut parsing::load_next_month_shifts(&driver, logbook)
            .await
            .context("Loading next month shifts")?,
    );
    info!("Found {} shifts", new_shifts.len());

    let mut force_replace = false;
    // If getting previous shift information failed, just create an empty one. Because it will cause a new calendar to be created
    let mut previous_shifts =
        match ical::get_previous_shifts().warn_owned("Getting previous shift information") {
            Ok(Err(ical::CalendarVersionError::ForceReplace)) => {
                warn!("Force replacing shifts");
                force_replace = true;
                ical::PreviousShifts::default()
            }
            Ok(Ok(previous_shifs)) => previous_shifs,
            _ => ical::PreviousShifts::default(),
        };
    non_relevant_shifts.append(&mut previous_shifts.non_relevant_shifts);
    let previous_relevant_shifts = previous_shifts.relevant_shifts;

    // The main send email function will return the broken shifts that are new or have changed.
    // This is because the send email functions uses the previous shifts and scans for new shifts
    let new_and_removed_shifts =
        shift::attach_shift_status(new_shifts, previous_relevant_shifts, force_replace);

    match email::send_emails(&new_and_removed_shifts).warn_owned("Sending shift emails") {
        Ok(_) => (),
        Err(_) => non_critical_error = Some(FailureType::EmailServer),
    }

    let non_relevant_shift_len = non_relevant_shifts.len();
    let mut all_shifts: Vec<Shift> = new_and_removed_shifts
        .into_iter()
        .filter(|shift| shift.state != ShiftState::Deleted)
        .collect();
    all_shifts.append(&mut non_relevant_shifts);

    let mut all_shifts_modified;
    if var("SKIP_BROKEN").unwrap_or_default() != "true" {
        all_shifts = gebroken_shifts::add_broken_shift_information(&driver, &all_shifts)
            .await
            .context("adding broken shift information")?; // Replace the shifts with the newly created list of broken shifts
        ical::save_partial_shift_files(&all_shifts).error("Saving partial shift files");
        all_shifts_modified = gebroken_shifts::split_broken_shifts(&all_shifts);
    } else {
        all_shifts_modified = all_shifts.clone();
    }

    if user.user_properties.stop_midnight_shift {
        all_shifts_modified = gebroken_shifts::stop_shift_at_midnight(&all_shifts_modified);
    }

    if user.user_properties.split_night_shift {
        all_shifts_modified = gebroken_shifts::split_night_shift(&all_shifts_modified)?;
    }

    all_shifts_modified.sort_by_key(|shift| shift.magic_number); // I do just just for peace of mind, it is probably not needed though
    all_shifts_modified.dedup();

    debug!("Saving {} shifts", all_shifts.len());
    let calendar = ical::create_calendar_file(&all_shifts_modified, &all_shifts, &logbook.state)
        .context("Creating calendar file")?;

    info!("Writing to: {:?}", &ical_path);
    write(ical_path, calendar.as_bytes())
        .await
        .context("Writing calendar file")?;

    if send_welcome {
        email::send_welcome_mail(false).context("Sending welcome mail")?;
    }

    logbook.generate_shift_statistics(&all_shifts, non_relevant_shift_len);
    Ok(non_critical_error)
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

    let name = data::get_set_name(None);
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
    let driver = match webdriver::get_driver(&mut logbook).await {
        Ok(driver) => driver,
        Err(err) => {
            error!("Failed to get driver! error: {}", err.to_string());
            current_exit_code = FailureType::GeckoEngine;
            clean_execution(&mut logbook, &current_exit_code, sender).await;
            return current_exit_code;
        }
    };

    while retry_count < max_retry_count && allow_execution {
        match main_program(&driver, retry_count, &mut logbook, sender.clone())
            .await
            .warn_owned("Main Program")
        {
            Ok(Some(non_crit)) => current_exit_code = non_crit,
            Ok(_) => {
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
        email::send_errors(&running_errors, &name).warn("Sending errors in loop");
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
