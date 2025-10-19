const BASE_DIRECTORY: &str = "kuma/";
const MAIN_URL: &str = "webcom.connexxion.nl";
// the ;x should be equal to the ammount of fallback URLs
const FALLBACK_URL: [&str; 2] = [
    "https://dmz-wbc-web01.connexxion.nl/WebComm/default.aspx",
    "https://dmz-wbc-web02.connexxion.nl/WebComm/default.aspx",
];

extern crate pretty_env_logger;
#[macro_use]
extern crate log;

use clap::Parser;
use clap::command;
use dotenvy::dotenv_override;
use dotenvy::var;
use email::send_errors;
use email::send_welcome_mail;
use sea_orm::Database;
use std::cell::RefCell;
use std::fs;
use std::fs::write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::RwLock;
use thirtyfour::prelude::*;
use time::macros::format_description;
use tokio::spawn;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::channel;

use crate::errors::FailureType;
use crate::errors::IncorrectCredentialsCount;
use crate::errors::ResultLog;
use crate::errors::SignInFailure;
use crate::execution::StartReason;
use crate::execution::execution_manager;
use crate::execution::start_pipe;
use crate::health::ApplicationLogbook;
use crate::health::send_heartbeat;
use crate::health::update_calendar_exit_code;
use crate::ical::*;
use crate::parsing::*;
use crate::shift::*;
use crate::variables::UserInstanceData;
use crate::variables::GeneralProperties;
use crate::variables::UserData;
use crate::watchdog::watchdog;

pub mod email;
pub mod errors;
mod execution;
pub mod gebroken_shifts;
mod health;
mod ical;
pub mod kuma;
mod parsing;
pub mod shift;
mod variables;
mod watchdog;

type GenResult<T> = Result<T, GenError>;
type GenError = Box<dyn std::error::Error + Send + Sync + 'static>;

static NAME: LazyLock<RwLock<Option<String>>> = LazyLock::new(|| RwLock::new(None));

thread_local! {
    pub static USER_PROPERTIES: RefCell<Option<Arc<UserData>>> = RefCell::new(None);
    pub static GENERAL_PROPERTIES: RefCell<Option<Arc<GeneralProperties>>> = RefCell::new(None);
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    instant_run: bool,
    #[arg(short, long)]
    single_run: bool,
}

fn create_shift_link(shift: &Shift, include_domain: bool) -> GenResult<String> {
    let (_user, properties) = get_instance();
    let date_format = format_description!("[day]-[month]-[year]");
    let formatted_date = shift.date.format(date_format)?;
    let domain = match include_domain {
        true => &properties.pdf_shift_domain,
        false => "",
    };
    if domain.is_empty() && include_domain == true {
        return Ok(format!(
            "https://dmz-wbc-web01.connexxion.nl/WebComm/shiprint.aspx?{}",
            &formatted_date
        ));
    }
    let shift_number_bare = match shift.number.split("-").next() {
        Some(shift_number) => shift_number,
        None => return Err("Could not get shift number".into()),
    };
    Ok(format!(
        "{domain}{shift_number_bare}?date={}",
        &formatted_date
    ))
}

fn create_ical_filename() -> String {
    let (user, _properties) = get_instance();
    match &user.file_name {
        value if value.is_empty() => format!("{}.ics", user.user_name),
        _ => format!("{}.ics", user.file_name),
    }
}

pub async fn wait_until_loaded(driver: &WebDriver) -> GenResult<()> {
    let mut started_loading = false;
    let timeout_duration = std::time::Duration::from_secs(30);
    let _ = tokio::time::timeout(timeout_duration, async {
        loop {
            let ready_state: ScriptRet =
                driver.execute("return document.readyState", vec![]).await?;
            let current_state = format!("{:?}", ready_state.json());
            if current_state == "String(\"complete\")" && started_loading {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                return Ok::<(), WebDriverError>(());
            }
            if current_state == "String(\"loading\")" {
                started_loading = true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

async fn wait_untill_redirect(driver: &WebDriver) -> GenResult<()> {
    let initial_url = driver.current_url().await?;
    let mut current_url = driver.current_url().await?;
    let timeout = std::time::Duration::from_secs(30); // Maximum wait time.

    tokio::time::timeout(timeout, async {
        loop {
            let new_url = driver.current_url().await.unwrap();
            if new_url != current_url {
                current_url = new_url;
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;

    if current_url == initial_url {
        warn!("Timeout waiting for redirect.");
        return Err(Box::new(WebDriverError::Timeout(
            "Redirect did not occur".into(),
        )));
    }

    debug!("Redirected to: {}", current_url);
    wait_until_loaded(driver).await?;
    Ok(())
}

pub fn create_path(filename: &str) -> PathBuf {
    let (_user, properties) = get_instance();
    let mut path = PathBuf::from(&properties.file_target);
    path.push(filename);
    path
}

fn set_get_name(set_new_name: Option<String>) -> String {
    let path = create_path("name");
    // Just return constant name if already set
    if let Ok(const_name_option) = NAME.read() {
        if let Some(const_name) = const_name_option.clone()
            && set_new_name.is_none()
        {
            return const_name;
        }
    }
    let mut name = std::fs::read_to_string(&path)
        .ok()
        .unwrap_or("FOUT BIJ LADEN VAN NAAM".to_owned());

    // Write new name if previous name is different (deadname protection lmao)
    if let Some(new_name) = set_new_name
        && new_name != name
    {
        write(&path, &new_name).error("Opslaan van naam");
        name = new_name;
    }
    if let Ok(mut const_name) = NAME.write() {
        *const_name = Some(name.clone());
    }
    name
}

pub fn get_instance() -> (Arc<UserData>, Arc<GeneralProperties>) {
    let user = USER_PROPERTIES
        .with_borrow(|user| user.as_ref().cloned())
        .expect("Failed to get UserData");
    let properties = GENERAL_PROPERTIES
        .with_borrow(|user| user.as_ref().cloned())
        .expect("Failed to get Properties");
    (user, properties)
}

// Main program logic that has to run, if it fails it will all be reran.
async fn main_program(
    driver: &WebDriver,
    retry_count: usize,
    logbook: &mut ApplicationLogbook,
) -> GenResult<()> {
    let (user, _properties) = get_instance();
    let personeelsnummer = &user.personeelsnummer;
    let password = &user.password;
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
    let ical_path = get_ical_path()?;
    if !ical_path.exists() {
        info!(
            "Existing calendar file not found, adding two extra months of shifts and removing partial calendars"
        );
        _ = || -> GenResult<()> {
            fs::remove_file(PathBuf::from(NON_RELEVANT_EVENTS_PATH))?;
            fs::remove_file(PathBuf::from(RELEVANT_EVENTS_PATH))?;
            Ok(())
        }();
        let found_shifts = load_previous_month_shifts(&driver, 2).await?;
        debug!("Found a total of {} shifts", found_shifts.len());
        let mut found_shifts_split = split_relevant_shifts(found_shifts);
        new_shifts.append(&mut found_shifts_split.0);
        non_relevant_shifts.append(&mut found_shifts_split.1);
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
    let mut previous_shifts_information = || -> Option<PreviousShiftInformation> {
        Some(
            get_previous_shifts()
                .warn_owned("Getting previous shift information")
                .ok()??,
        )
    }()
    .unwrap_or_default();
    non_relevant_shifts.append(&mut previous_shifts_information.previous_non_relevant_shifts);
    let previous_shifts = previous_shifts_information.previous_relevant_shifts;
    // The main send email function will return the broken shifts that are new or have changed.
    // This is because the send email functions uses the previous shifts and scanns for new shifts
    // write("./shifts.json",serde_json::to_string_pretty(&new_shifts).unwrap());
    let relevant_shifts = match email::send_emails(new_shifts, previous_shifts) {
        Ok(shifts) => shifts,
        Err(err) => return Err(err),
    };
    let mut all_shifts = relevant_shifts;
    let non_relevant_shift_len = non_relevant_shifts.len();
    all_shifts.append(&mut non_relevant_shifts);
    let all_shifts = gebroken_shifts::load_broken_shift_information(&driver, &all_shifts).await?; // Replace the shifts with the newly created list of broken shifts
    ical::save_partial_shift_files(&all_shifts).error("Saving partial shift files");
    let broken_split_shifts = gebroken_shifts::split_broken_shifts(&all_shifts);
    let midnight_stopped_shifts = gebroken_shifts::stop_shift_at_midnight(&broken_split_shifts)?;
    let mut night_split_shifts = gebroken_shifts::split_night_shift(&midnight_stopped_shifts)?;
    night_split_shifts.sort_by_key(|shift| shift.magic_number);
    night_split_shifts.dedup();
    debug!("Saving {} shifts", night_split_shifts.len());
    let calendar = create_ical(&night_split_shifts, &all_shifts, &logbook.state);
    send_welcome_mail(&ical_path, false)?;
    info!("Writing to: {:?}", &ical_path);
    write(ical_path, calendar?.as_bytes())?;
    logbook.generate_shift_statistics(&all_shifts, non_relevant_shift_len);
    Ok(())
}

async fn initiate_webdriver() -> GenResult<WebDriver> {
    let gecko_ip = var("GECKO_IP")?;
    let caps = DesiredCapabilities::firefox();
    let driver = WebDriver::new(format!("http://{}", gecko_ip), caps).await?;
    Ok(driver)
}

// Create file on disk to show webcom ical is currently active
// Always delete the file at the beginning of this function
// Only create a new file if start reason is Some
fn create_delete_lock(start_reason: Option<&StartReason>) -> GenResult<()> {
    let path = create_path("active");
    if path.exists() {
        debug!("Removing existing lock file");
        fs::remove_file(&path)?;
    }
    if let Some(start_reason) = start_reason {
        debug!("Creating new lock file");
        let text = serde_json::to_string(start_reason).unwrap_or_default();
        write(&path, text.as_bytes())?;
    }
    Ok(())
}

/*
This starts the WebDriver session
Loads the main logic, and retries if it fails
*/
async fn main_loop(receiver: &mut Receiver<StartReason>, instance: UserInstanceData) {
    loop {
        debug!("Waiting for notification");
        let continue_execution = receiver.recv().await.expect("Notification channel closed");

        USER_PROPERTIES.replace(Some(instance.user_data.load_full()));
        GENERAL_PROPERTIES.replace(Some(instance.general_settings.load_full()));
        let (_user, properties) = get_instance();

        create_delete_lock(Some(&continue_execution)).warn("Creating Lock file");

        let name = set_get_name(None);
        let mut logbook = ApplicationLogbook::load();
        let mut failure_counter = IncorrectCredentialsCount::load();

        let driver = match get_driver(&mut logbook).await {
            Ok(driver) => driver,
            Err(err) => {
                error!("Failed to get driver! error: {}", err.to_string());
                logbook
                    .save(&FailureType::GeckoEngine)
                    .warn("Saving gecko driver error");
                return ();
            }
        };

        let mut current_exit_code = FailureType::default();
        let previous_exit_code = logbook.clone().state;
        let mut running_errors: Vec<GenError> = vec![];

        let mut retry_count: usize = 0;
        let max_retry_count: usize = properties.execution_retry_count as usize;

        // Check if the program is allowed to run, or not due to failed sign-in
        let sign_in_check: Option<SignInFailure> =
            failure_counter.sign_in_failed_check().unwrap_or(None);
        if continue_execution != StartReason::Force {
            if let Some(failure) = sign_in_check {
                retry_count = max_retry_count;
                current_exit_code = FailureType::SignInFailed(failure);
            }
        } else {
            info!("Force resuming execution");
        }

        while retry_count < max_retry_count {
            match main_program(&driver, retry_count, &mut logbook)
                .await
                .warn_owned("Main Program")
            {
                Ok(()) => {
                    failure_counter
                        .update_signin_failure(false, None)
                        .warn("Updating signin failure");
                    retry_count = max_retry_count;
                }
                Err(err) if err.downcast_ref::<FailureType>().is_some() => {
                    let webcom_error = err
                        .downcast_ref::<FailureType>()
                        .cloned()
                        .unwrap_or_default();
                    match webcom_error.clone() {
                        FailureType::SignInFailed(signin_failure) => {
                            retry_count = max_retry_count;
                            failure_counter
                                .update_signin_failure(true, Some(signin_failure.clone()))
                                .warn("Updating signin failure 2");
                            current_exit_code = webcom_error;
                        }
                        FailureType::ConnectError => {
                            retry_count = max_retry_count;
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

        if current_exit_code != FailureType::TriesExceeded {
            send_heartbeat(&current_exit_code)
                .await
                .warn("Sending Heartbeat in loop");
        }

        logbook
            .save(&current_exit_code)
            .warn("Saving logbook in loop");

        // Update the exit code in the calendar if it is not equal to the previous value
        if previous_exit_code != current_exit_code {
            warn!("Previous exit code was different than current, need to update");
            update_calendar_exit_code(&previous_exit_code, &current_exit_code)
                .warn("Updating calendar exit code");
        }

        create_delete_lock(None).warn("Removing Lock file");

        if continue_execution == StartReason::Single {
            break;
        }
    }
}

async fn get_driver(logbook: &mut ApplicationLogbook) -> GenResult<WebDriver> {
    match initiate_webdriver().await {
        Ok(driver) => Ok(driver),
        Err(error) => {
            error!("Kon driver niet opstarten: {:?}", &error);
            send_errors(&vec![error], &set_get_name(None)).info("Send errors");
            logbook
                .save(&FailureType::GeckoEngine)
                .warn("Saving Logbook");
            send_heartbeat(&FailureType::GeckoEngine)
                .await
                .warn("Sending heartbeat");
            return Err("driver fout".into());
        }
    }
}

#[tokio::main]
async fn main() -> GenResult<()> {
    dotenv_override().ok();
    pretty_env_logger::init();
    info!("Starting Webcom Ical");

    let args = Args::parse();

    let db = Database::connect(&var("DB_URL")?)
        .await
        .unwrap();
    let user = UserInstanceData::load_user(&db, "25348")
        .await?
        .expect("No user found");
    watchdog(&db).await?;

    let (tx, mut rx) = channel(1);
    let tx_clone = tx.clone();
    let instant_run = args.instant_run;
    // If the single run argument is set, just send a single message so the main loop instantly runs.
    // Otherwise start the execution manager
    match args.single_run {
        false => {
            spawn(async move { execution_manager(tx, instant_run).await });
        }
        true => {
            tx.send(StartReason::Single).await?;
        }
    };
    let main_program = spawn(async move { main_loop(&mut rx, user).await });
    if !args.single_run {
        start_pipe(tx_clone).await.warn("Start pipe");
    } else {
        main_program.await.info("Main program");
    }
    info!("Stopping webcom ical");
    Ok(())
}

// OLD CODE
// if let Some(kuma_url) = kuma_url.clone()
//     && !kuma_url.is_empty()
// {
//     debug!("Checking if kuma needs to be created");
//     kuma::first_run(&kuma_url, &username).await.warn("Kuma Run");
// }
