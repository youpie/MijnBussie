use crate::GenResult;
use crate::api::auth::check_api_key;
use crate::errors::OptionResult;
use crate::execution::timer::StartRequest;
use crate::execution::watchdog::{InstanceMap, RequestResponse, WatchdogRequest};
use crate::kuma::{KumaAction, KumaUserRequest};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router, middleware};
use axum_server::tls_rustls::RustlsConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use strum_macros::EnumString;
use tokio::sync::RwLock;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::timeout;
use tracing::info;

#[derive(Clone)]
pub struct ServerConfig {
    map: Arc<RwLock<InstanceMap>>,
    sender: Sender<WatchdogRequest>,
}

#[derive(Clone, EnumString, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "snake_case"))]
enum Action {
    Logbook,
    IsActive,
    Name,
    Start,
    ExitCode,
    UserData,
    Welcome,
    Calendar,
    Delete,
    Standing,
}

pub async fn api(instance_map: Arc<RwLock<InstanceMap>>, watchdog_sender: Sender<WatchdogRequest>) {
    let config = ServerConfig {
        map: instance_map,
        sender: watchdog_sender,
    };

    let tls_config = RustlsConfig::from_pem_file(
        PathBuf::from("cert").join("cert.crt"),
        PathBuf::from("cert").join("key.key"),
    )
    .await
    .expect("Missing certificate files");
    let api_routes = Router::new()
        .route("/{user_name}/{action}", get(get_information))
        .route("/refresh", get(refresh_users))
        .route("/refresh/{user_name}", get(refresh_users))
        .route("/kuma/{action}/{user_name}", get(handle_kuma_request))
        .layer(middleware::from_fn(check_api_key))
        .with_state(config);

    let all_routes = Router::new().nest("/api", api_routes);

    axum_server::bind_rustls(
        std::net::SocketAddr::from_str("0.0.0.0:3000").unwrap(),
        tls_config,
    )
    .serve(all_routes.into_make_service())
    .await
    .unwrap();
}

async fn refresh_users(
    State(data): State<ServerConfig>,
    user_name: Option<Path<String>>,
) -> impl IntoResponse {
    let send = data
        .sender
        .try_send(
            user_name
                .and_then(|path| Some(WatchdogRequest::SingleUser(path.to_string())))
                .unwrap_or(WatchdogRequest::AllUser),
        )
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, Json(err.to_string())));
    send.into_response()
}

async fn get_information(
    State(data): State<ServerConfig>,
    Path((user_name, action)): Path<(String, Action)>,
) -> impl IntoResponse {
    match data.map.read().await.get(&user_name) {
        Some(instance) => {
            match send_request(
                action,
                &instance.request_sender,
                &mut *instance.response_receiver.write().await,
            )
            .await
            {
                Ok(response) => (StatusCode::OK, Json(response)).into_response(),
                Err(err) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(err.to_string())).into_response()
                }
            }
        }
        None => (StatusCode::BAD_REQUEST, Json("User not found".to_string())).into_response(),
    }
}

async fn send_request(
    action: Action,
    request_sender: &Sender<StartRequest>,
    response_receiver: &mut Receiver<RequestResponse>,
) -> GenResult<RequestResponse> {
    let start_request = match action {
        Action::Logbook => StartRequest::Logbook,
        Action::IsActive => StartRequest::IsActive,
        Action::Name => StartRequest::Name,
        Action::Start => StartRequest::Api,
        Action::ExitCode => StartRequest::ExitCode,
        Action::UserData => StartRequest::UserData,
        Action::Welcome => StartRequest::Welcome,
        Action::Calendar => StartRequest::Calendar,
        Action::Delete => StartRequest::Delete,
        Action::Standing => StartRequest::Standing,
    };
    request_sender.try_send(start_request)?;
    let response = timeout(Duration::from_secs(2), response_receiver.recv())
        .await?
        .result_reason("No response")?;

    Ok(response)
}

async fn handle_kuma_request(
    State(data): State<ServerConfig>,
    Path((action, user_name)): Path<(KumaAction, String)>,
) -> impl IntoResponse {
    info!("Kuma request");
    match handle_kuma(data.sender, user_name, action).await {
        Ok(_) => (StatusCode::OK, Json("OK".to_string())),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, Json(err.to_string())),
    }
}

async fn handle_kuma(
    channel: Sender<WatchdogRequest>,
    user_name: String,
    action: KumaAction,
) -> GenResult<()> {
    let user_name = match user_name {
        user if user == "all" => KumaUserRequest::All,
        user => KumaUserRequest::Users(vec![user]),
    };
    let kuma_request = (action, user_name);
    channel.try_send(WatchdogRequest::KumaRequest(kuma_request))?;
    Ok(())
}
