const MAIN_URL: &str = "webcom.connexxion.nl";
// the ;x should be equal to the ammount of fallback URLs
const FALLBACK_URL: [&str; 2] = [
    "https://dmz-wbc-web01.connexxion.nl/WebComm/default.aspx",
    "https://dmz-wbc-web02.connexxion.nl/WebComm/default.aspx",
];
const APPLICATION_NAME: &str = "Mijn Bussie";

use crate::api::route::api;
use crate::database::secret::Secret;
use crate::database::variables::GeneralProperties;
use crate::database::variables::UserData;
use crate::database::variables::UserInstanceData;
use crate::errors::FailureType;
use crate::errors::ResultLog;
use crate::errors::SignInFailure;
use crate::errors::ToString;
use crate::execution::timer::StartRequest;
use crate::execution::timer::execution_timer;
use crate::execution::watchdog::WatchdogRequest;
use crate::execution::watchdog::watchdog;
use crate::execution::watchdog::{InstanceMap, RequestResponse};
use crate::health::ApplicationLogbook;
use crate::webcom::deletion::StandingInformation;
use crate::webcom::deletion::check_instance_standing;
use crate::webcom::deletion::delete_account;
use crate::webcom::deletion::update_instance_timestamps;
use crate::webcom::email;
use crate::webcom::email::create_calendar_link;
use crate::webcom::ical::get_ical_path;
use crate::webcom::shift::*;
use crate::webcom::webcom::webcom_instance;
use dotenvy::dotenv_override;
use dotenvy::var;
use entity::user_data;
use migration::Migrator;
use migration::MigratorTrait;
use rustls::crypto::CryptoProvider;
use rustls::crypto::ring::default_provider;
use sea_orm::ActiveValue::Set;
use sea_orm::Database;
use sea_orm::DatabaseConnection;
use sea_orm::EntityTrait;
use sea_orm::IntoActiveModel;
use secrecy::ExposeSecret;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::set_permissions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use time::macros::format_description;
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tokio::sync::mpsc::channel;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tokio::task_local;
use tokio::time::sleep;
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
    let (user, _properties) = get_data();
    get_set_name_local(user.as_ref(), set_new_name)
}

pub fn get_set_name_local(user: &UserData, set_new_name: Option<String>) -> String {
    // To get the name, first try the new name function body variable.
    // Then try the global variable
    // Then try the Local database variable (which is not set the first time the instance is ever run)
    // So if this is called before the first time the instance is run, it wil return "Onbekend"
    let name = set_new_name
        .as_deref()
        .unwrap_or(
            NAME.get().borrow().as_deref().unwrap_or(
                user.name
                    .as_ref()
                    .and_then(|secret| Some(secret.0.expose_secret()))
                    .unwrap_or(&user.user_name),
            ),
        )
        .to_owned();

    // Open a database connection and write the new name to the database, if a new name request is done
    if let Some(new_name) = set_new_name
        && Some(new_name.as_str()) != NAME.get().borrow().as_deref()
    {
        tokio::task::block_in_place(move || {
            Handle::current().block_on(update_name(new_name, user.id))
        })
        .warn("Setting name");
    }
    NAME.get().replace(Some(name.clone()));
    name
}

async fn update_name(new_name: String, data_id: i32) -> GenResult<()> {
    info!("Changing user name to {new_name}");
    let db = get_database_connection().await;
    let data = user_data::Entity::find_by_id(data_id).one(&db).await?;
    if let Some(model) = data {
        let mut active_model = model.into_active_model();
        active_model.name = Set(Some(Secret::encrypt_value(&new_name)?));
        user_data::Entity::update(active_model)
            .validate()?
            .exec(&db)
            .await?;
        Ok(())
    } else {
        Err("UserData not found".into())
    }
}

/// If Webcom is running
/// Return false
/// if it is not
/// get the exit code of the previous join handle and set it
/// spawn a new webcom instance
async fn spawn_webcom_instance(
    start_request: &StartRequest,
    exit_code_sender: Arc<Sender<StartRequest>>,
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
                    NAME.scope(
                        RefCell::new(None),
                        webcom_instance(start_request.clone(), exit_code_sender),
                    ),
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
    meta_sender: Arc<Sender<StartRequest>>,
    instance: UserInstanceData,
) {
    let (_user, _properties) = set_data(&instance).await;
    let tracer = tracing_appender::rolling::daily(create_path("logs"), "log");

    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env()
        .unwrap();

    let (non_blocking, _guard) = non_blocking::NonBlocking::new(tracer);

    let subscriber = Arc::new(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(non_blocking)
            .with_env_filter(filter)
            .finish(),
    );
    debug!("starting");

    let mut receiver = receiver;
    let mut webcom_thread: Option<JoinHandle<FailureType>> = None;
    let mut last_exit_code = FailureType::default();
    let mut instance_active = true;

    while instance_active {
        debug!("Waiting for notification");
        let start_request = receiver.recv().await.expect("Notification channel closed");

        let (user, _properties) = set_data(&instance).await;
        info!("Recieved {start_request:?} request");
        let response = match start_request {
            StartRequest::Logbook => Some(RequestResponse::Logbook(ApplicationLogbook::load())),
            StartRequest::Name => Some(RequestResponse::Name(get_set_name(None))),
            StartRequest::IsActive => Some(RequestResponse::Active(is_webcom_instance_active(
                &webcom_thread,
            ))),
            StartRequest::Api => Some(RequestResponse::Active(
                spawn_webcom_instance(
                    &start_request,
                    meta_sender.clone(),
                    &mut webcom_thread,
                    &mut last_exit_code,
                )
                .with_subscriber(subscriber.clone())
                .await,
            )),
            StartRequest::ExitCode => Some(RequestResponse::ExitCode(last_exit_code.clone())),
            StartRequest::UserData => Some(RequestResponse::UserData(user.as_ref().clone())),
            StartRequest::Welcome => Some(RequestResponse::GenResponse(
                email::send_welcome_mail(&get_ical_path(), true).to_string(),
            )),
            StartRequest::Calendar => return_calendar_response(),
            StartRequest::ExecutionFinished(ref exit_code) => {
                update_instance_timestamps(exit_code, instance.user_data.clone())
                    .await
                    .warn("Updating instance timestamps");
                check_instance_standing().await;
                log_exit_code(exit_code, &last_exit_code)
            }
            StartRequest::Delete => {
                instance_active = false;
                _ = webcom_thread.as_ref().is_some_and(|thread| {
                    thread.abort();
                    true
                });
                Some(RequestResponse::GenResponse(
                    delete_account(user.id, email::DeletedReason::Manual)
                        .await
                        .to_string(),
                ))
            }
            StartRequest::Standing => {
                Some(RequestResponse::InstanceStanding(StandingInformation::get()))
            }
            _ => {
                spawn_webcom_instance(
                    &start_request,
                    meta_sender.clone(),
                    &mut webcom_thread,
                    &mut last_exit_code,
                )
                .with_subscriber(subscriber.clone())
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
    warn!("Killing instance, bye👋");
    sleep(Duration::from_hours(12)).await;
    warn!("Manually killing instance after waiting");
}

fn log_exit_code(exit_code: &FailureType, last_exit_code: &FailureType) -> Option<RequestResponse> {
    let failed_signin_type = &FailureType::SignInFailed(SignInFailure::IncorrectCredentials);
    if exit_code == failed_signin_type {
        if last_exit_code != failed_signin_type {
            warn!("Signin no longer succesful");
        }
    } else if exit_code != &FailureType::OK {
        warn!("Exited with non-OK exit code: {exit_code:?}");
    }
    None
}

fn return_calendar_response() -> Option<RequestResponse> {
    match create_calendar_link() {
        Ok(link) => Some(RequestResponse::GenResponse(link.to_string())),
        Err(_) => None,
    }
}

pub fn set_strict_file_permissions(path: &PathBuf) -> GenResult<()> {
    let file = std::fs::File::open(&path)?;
    let metadata = file.metadata()?;
    let mut file_mode = metadata.permissions();
    file_mode.set_mode(0o100600);
    set_permissions(&path, file_mode)?;
    Ok(())
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

async fn get_database_connection() -> DatabaseConnection {
    Database::connect(&var("DATABASE_URL").expect("Failed to get database URL"))
        .await
        .expect("Could not connect to database")
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
    CryptoProvider::install_default(default_provider()).unwrap();
    // let args = Args::parse();

    let db = get_database_connection().await;

    // Apply all pending migrations
    Migrator::up(&db, None).await?;

    let (watchdog_tx, mut watchdog_rx) = channel(1);
    _ = watchdog_tx.try_send(WatchdogRequest::AllUser);

    let instances: Arc<RwLock<InstanceMap>> = Arc::new(RwLock::new(HashMap::new()));

    tokio::spawn(execution_timer(instances.clone()));
    tokio::spawn(api(instances.clone(), watchdog_tx));

    watchdog(instances.clone(), &db, &mut watchdog_rx).await?;

    info!("Stopping {APPLICATION_NAME}");
    Ok(())
}
