const MAIN_URL: &str = "webcom.connexxion.nl";
// the ;x should be equal to the ammount of fallback URLs
const FALLBACK_URL: [&str; 2] = [
    "https://dmz-wbc-web01.connexxion.nl/WebComm/default.aspx",
    "https://dmz-wbc-web02.connexxion.nl/WebComm/default.aspx",
];
const APPLICATION_NAME: &str = "Mijn Bussie";

use crate::api::route::api;
use crate::database::variables::GeneralProperties;
use crate::database::variables::UserData;
use crate::database::variables::UserInstanceData;
use crate::errors::FailureType;
use crate::errors::ResultLog;
use crate::errors::SignInFailure;
use crate::execution::timer::StartRequest;
use crate::execution::timer::execution_timer;
use crate::execution::watchdog::watchdog;
use crate::execution::watchdog::{InstanceMap, RequestResponse};
use crate::health::ApplicationLogbook;
use crate::webcom::email;
use crate::webcom::ical::get_ical_path;
use crate::webcom::shift::*;
use crate::webcom::webcom::webcom_instance;
use dotenvy::dotenv_override;
use dotenvy::var;
use migration::Migrator;
use migration::MigratorTrait;
use sea_orm::Database;
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use time::macros::format_description;
use tokio::fs::write;
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tokio::sync::mpsc::channel;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tokio::task_local;
use tracing::instrument::WithSubscriber;
use tracing::level_filters::LevelFilter;
use tracing::*;
use tracing_appender::non_blocking;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;

mod api;
mod database;
mod errors;
mod execution;
mod health;
mod kuma;
mod webcom;

type GenResult<T> = Result<T, GenError>;
type GenError = Box<dyn std::error::Error + Send + Sync + 'static>;

task_local! {
    static NAME: RefCell<Option<String>>;
    static USER_PROPERTIES: RefCell<Option<Arc<UserData>>>;
    static GENERAL_PROPERTIES: RefCell<Option<Arc<GeneralProperties>>>;
}

// Get thread specific data
pub fn get_data() -> (Arc<UserData>, Arc<GeneralProperties>) {
    let user = USER_PROPERTIES.with(|data| data.borrow().clone().expect("Failed to get UserData"));
    let properties =
        GENERAL_PROPERTIES.with(|data| data.borrow().clone().expect("Failed to get Properties"));
    (user, properties)
}

// Sets thread specific data, also returns new values
async fn set_data(instance: &UserInstanceData) -> (Arc<UserData>, Arc<GeneralProperties>) {
    let user_data = Arc::new(instance.user_data.read().await.clone());
    let settings_data = Arc::new(instance.general_settings.read().await.clone());
    USER_PROPERTIES.with(|data| *data.borrow_mut() = Some(user_data.clone()));
    GENERAL_PROPERTIES.with(|data| *data.borrow_mut() = Some(settings_data.clone()));
    (user_data, settings_data)
}

fn create_shift_link(shift: &Shift, include_domain: bool) -> GenResult<String> {
    let (_user, properties) = get_data();
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
    let (user, _properties) = get_data();
    match &user.file_name {
        value if value.is_empty() => format!("{}.ics", user.user_name),
        _ => format!("{}.ics", user.file_name),
    }
}

pub fn create_path_local(
    user: &UserData,
    properties: &GeneralProperties,
    filename: &str,
) -> PathBuf {
    let mut path = PathBuf::from(&properties.file_target);
    path.push(&user.user_name);
    std::fs::create_dir_all(&path).warn("Creating dirs");
    path.push(filename);
    path
}

pub fn create_path(filename: &str) -> PathBuf {
    let (user, properties) = get_data();
    create_path_local(user.as_ref(), properties.as_ref(), filename)
}

fn get_set_name(set_new_name: Option<String>) -> String {
    let (user, properties) = get_data();
    get_set_name_local(user.as_ref(), properties.as_ref(), set_new_name)
}

pub fn get_set_name_local(
    user: &UserData,
    properties: &GeneralProperties,
    set_new_name: Option<String>,
) -> String {
    let path = create_path_local(user, properties, "name");
    // Just return constant name if already set
    if let Some(const_name) = &*NAME.get().borrow()
        && set_new_name.is_none()
    {
        return const_name.to_owned();
    }
    let mut name = std::fs::read_to_string(&path)
        .ok()
        .unwrap_or("Onbekend".to_owned());

    // Write new name if previous name is different (deadname protection lmao)
    if let Some(new_name) = set_new_name
        && new_name != name
    {
        let new_name_clone = new_name.clone();
        tokio::task::block_in_place(move || {
            Handle::current().block_on(write(&path, &new_name_clone))
        })
        .error("Opslaan van naam");
        name = new_name;
    }
    NAME.get().replace(Some(name.clone()));
    name
}

/// If Webcom is running
/// Return false
/// if it is not
/// get the exit code of the previous join handle and set it
/// spawn a new webcom instance
async fn spawn_webcom_instance(
    start_request: StartRequest,
    thread_store: &mut Option<JoinHandle<FailureType>>,
    last_exit_code: &mut FailureType,
) -> bool {
    if let Some(thread) = thread_store
        && !thread.is_finished()
    {
        return false;
    } else if let Some(thread) = thread_store {
        *last_exit_code = thread.await.unwrap_or_default();
    }
    let (user, properties) = get_data();
    *thread_store = Some(tokio::spawn(
        USER_PROPERTIES
            .scope(
                RefCell::new(Some(user)),
                GENERAL_PROPERTIES.scope(
                    RefCell::new(Some(properties)),
                    NAME.scope(RefCell::new(None), webcom_instance(start_request)),
                ),
            )
            .with_current_subscriber(),
    ));
    true
}

fn is_webcom_instance_active(thread_store: &Option<JoinHandle<FailureType>>) -> bool {
    thread_store
        .as_ref()
        .is_some_and(|thread| !thread.is_finished())
}

/*
This starts the WebDriver session
Loads the main logic, and retries if it fails
*/
async fn user_instance(
    receiver: Receiver<StartRequest>,
    sender: Sender<RequestResponse>,
    instance: UserInstanceData,
) {
    let (_user, _properties) = set_data(&instance).await;
    let tracer = tracing_appender::rolling::daily(create_path("logs"), "log");

    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env()
        .unwrap();

    let (non_blocking, _guard) = non_blocking::NonBlocking::new(tracer);

    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(non_blocking)
        .with_env_filter(filter)
        .finish();
    async {
        info!("starting");
        let mut receiver = receiver;
        let mut webcom_thread: Option<JoinHandle<FailureType>> = None;
        let mut last_exit_code = FailureType::default();
        loop {
            debug!("Waiting for notification");
            let start_request = receiver.recv().await.expect("Notification channel closed");

            let (user, _properties) = set_data(&instance).await;

            let response = match start_request {
                StartRequest::Logbook => Some(RequestResponse::Logbook(ApplicationLogbook::load())),
                StartRequest::Name => Some(RequestResponse::Name(get_set_name(None))),
                StartRequest::IsActive => Some(RequestResponse::Active(is_webcom_instance_active(
                    &webcom_thread,
                ))),
                StartRequest::Api => Some(RequestResponse::Active(
                    spawn_webcom_instance(start_request, &mut webcom_thread, &mut last_exit_code)
                        .with_current_subscriber()
                        .await,
                )),
                StartRequest::ExitCode => Some(RequestResponse::ExitCode(last_exit_code.clone())),
                StartRequest::UserData => Some(RequestResponse::UserData(user.as_ref().clone())),
                StartRequest::Welcome => Some(RequestResponse::GenResponse(format!(
                    "{:?}",
                    email::send_welcome_mail(&get_ical_path(), true)
                ))),
                _ => {
                    spawn_webcom_instance(start_request, &mut webcom_thread, &mut last_exit_code)
                        .with_current_subscriber()
                        .await;
                    None
                }
            };
            if let Some(response) = response {
                sender.try_send(response).info("Send response");
            }

            if start_request == StartRequest::Single {
                break;
            }
        }
    }
    .with_subscriber(subscriber)
    .await;
}

fn check_env_permissions() -> GenResult<()> {
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid())?;
    let permissions_target = 0o100600;
    let metadata = std::fs::File::open("./.env")?.metadata()?;
    let file_mode = metadata.permissions().mode();
    let file_owner = metadata.uid();
    if file_mode == permissions_target && file_owner == uid {
        Ok(())
    } else {
        Err(format!(
            "INCORRECT PERMISSIONS FOR ENV. Should be {permissions_target:o}, is {file_mode:o}. File owner should be {uid}, is {file_owner}"
        )
        .into())
    }
}

#[tokio::main]
async fn main() -> GenResult<()> {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()?;

    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_filter(filter);

    let global_subscriber = Registry::default().with(stdout_layer);
    tracing::subscriber::set_global_default(global_subscriber)
        .expect("Failed to set global subscriber");
    check_env_permissions().unwrap();

    dotenv_override()?;
    info!("Starting {APPLICATION_NAME}");

    // let args = Args::parse();

    let db = Database::connect(&var("DATABASE_URL")?)
        .await
        .expect("Could not connect to database");

    // Apply all pending migrations
    Migrator::up(&db, None).await?;

    let (watchdog_tx, mut watchdog_rx) = channel(1);
    _ = watchdog_tx.try_send("".to_owned());

    let instances: Arc<RwLock<InstanceMap>> = Arc::new(RwLock::new(HashMap::new()));

    tokio::spawn(execution_timer(instances.clone()));
    tokio::spawn(api(instances.clone(), watchdog_tx, db.clone()));

    watchdog(instances.clone(), &db, &mut watchdog_rx).await?;

    info!("Stopping {APPLICATION_NAME}");
    Ok(())
}
