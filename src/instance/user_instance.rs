use std::fs::read_to_string;

use tokio::task::spawn_blocking;
use tracing::instrument::WithSubscriber;
use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking;
use tracing_subscriber::EnvFilter;

use super::deletion::*;
use super::webcom::*;
use super::*;

/*
This starts the WebDriver session
Loads the main logic, and retries if it fails
*/
pub async fn user_instance(
    mut receiver: Receiver<StartRequest>,
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

    let mut system_request = false;

    let mut webcom_thread: Option<(JoinHandle<FailureType>, bool)> = None; // Bool = signedin
    let mut last_exit_code = ApplicationLogbook::load().state;
    let mut instance_active = true;

    while instance_active {
        debug!("Waiting for notification");
        let start_request = receiver.recv().await.expect("Notification channel closed");

        let (user, _properties) = set_data(&instance).await;
        info!("Recieved {start_request:?} request");
        let response = match start_request {
            StartRequest::Logbook => Some(RequestResponse::Logbook(ApplicationLogbook::load())),
            StartRequest::Name => Some(RequestResponse::Name(data::get_set_name(None))),
            StartRequest::IsActive => Some(RequestResponse::Active(is_webcom_instance_active(
                &webcom_thread,
            ))),
            StartRequest::Api => Some(RequestResponse::Started(
                webcom::spawn_webcom_instance(
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
            StartRequest::Welcome => {
                _ = spawn_blocking(|| email::send_welcome_mail(true))
                    .await
                    .and_then(|email_result| {
                        email_result.warn("Sending welcome email");
                        Ok(())
                    });
                Some(RequestResponse::GenResponse("OK".to_owned()))
            }
            StartRequest::Calendar => return_calendar_response(),
            StartRequest::Delete => {
                instance_active = false;

                _ = deletion::delete_account(user.id, DeletedReason::Manual)
                    .await
                    .warn("Account deletion");
                Some(RequestResponse::GenResponse("OK".to_owned()))
            }
            StartRequest::Standing => {
                Some(RequestResponse::InstanceStanding(StandingInformation::get()))
            }
            StartRequest::Logs => Some(RequestResponse::GenResponse(
                get_logfile().unwrap_or_else(|err| err.to_string()),
            )),
            StartRequest::ExecutionFinished(ref exit_code) => {
                deletion::update_instance_timestamps(
                    exit_code,
                    instance.user_data.clone(),
                    system_request,
                )
                .await
                .warn("Updating instance timestamps");
                system_request = false;
                deletion::check_instance_standing().await;
                last_exit_code = exit_code.clone();
                log_exit_code(exit_code, &last_exit_code)
            }
            StartRequest::SignedIn => {
                if let Some((_, signin)) = webcom_thread.as_mut() {
                    *signin = true;
                }
                None
            }
            _ => {
                system_request = true;
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
    _ = webcom_thread.as_ref().is_some_and(|thread| {
        thread.0.abort();
        true
    });
    // sleep(Duration::from_mins(40)).await;
    warn!("Manually killing instance after waiting");
}

fn return_calendar_response() -> Option<RequestResponse> {
    match ical::create_calendar_link() {
        Ok(link) => Some(RequestResponse::GenResponse(link.to_string())),
        Err(_) => None,
    }
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

pub fn get_instance_age(user: &UserData) -> i64 {
    let current_time = chrono::offset::Utc::now().naive_utc();
    user.creation_date
        .signed_duration_since(current_time)
        .num_days()
}

fn get_logfile() -> Result<String> {
    let path = create_path("logs");
    let last_modified_file = std::fs::read_dir(path)?
        .flatten() // Remove failed
        .filter(|f| f.metadata().unwrap().is_file()) // Filter out directories (only consider files)
        .max_by_key(|x| x.metadata().unwrap().modified().unwrap()); // Get the most recently modified file

    if let Some(log_file) = last_modified_file {
        return Ok(read_to_string(log_file.path())?);
    }
    Err(anyhow!("No Logfile"))
}
