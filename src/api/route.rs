use crate::database::variables::GeneralProperties;
use crate::errors::OptionResult;
use crate::timer::StartRequest;
use crate::watchdog::{InstanceMap, RequestResponse};
use crate::{GenResult, kuma};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use strum_macros::EnumString;
use tokio::sync::RwLock;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::timeout;
use tracing::*;
#[derive(Clone)]
pub struct ServerConfig {
    map: Arc<RwLock<InstanceMap>>,
    sender: Sender<String>,
    database: DatabaseConnection,
}

#[derive(Clone, EnumString, Debug, PartialEq, Serialize)]
enum Action {
    #[strum(ascii_case_insensitive)]
    Logbook,
    #[strum(ascii_case_insensitive)]
    IsActive,
    #[strum(ascii_case_insensitive)]
    Name,
    #[strum(ascii_case_insensitive)]
    Start,
    #[strum(ascii_case_insensitive)]
    ExitCode,
    #[strum(ascii_case_insensitive)]
    UserData,
    #[strum(ascii_case_insensitive)]
    Welcome,
}

pub async fn api(
    instance_map: Arc<RwLock<InstanceMap>>,
    watchdog_sender: Sender<String>,
    db: DatabaseConnection,
) {
    let config = ServerConfig {
        map: instance_map,
        sender: watchdog_sender,
        database: db,
    };
    let app = Router::new()
        .route("/api/{user_name}/{action}", get(get_information))
        .route("/api/refresh", get(refresh_users))
        .route("/api/refresh/{user_name}", get(refresh_users))
        .route("/api/kuma/{action}/{user_name}", get(handle_kuma_request))
        .with_state(config);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn refresh_users(
    State(data): State<ServerConfig>,
    user_name: Option<Path<String>>,
) -> impl IntoResponse {
    let send = data
        .sender
        .try_send(
            user_name
                .and_then(|path| Some(path.to_string()))
                .unwrap_or_default(),
        )
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
    send.into_response()
}

async fn get_information(
    State(data): State<ServerConfig>,
    Path((user_name, action)): Path<(String, String)>,
) -> impl IntoResponse {
    info!("Got a request for {user_name}");

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
        None => (StatusCode::NOT_FOUND, "User not found".to_string()).into_response(),
    }
}

async fn send_request(
    request_type: String,
    request_sender: &Sender<StartRequest>,
    response_receiver: &mut Receiver<RequestResponse>,
) -> GenResult<RequestResponse> {
    let action: Action = Action::from_str(&request_type)?;
    let start_request = match action {
        Action::Logbook => StartRequest::Logbook,
        Action::IsActive => StartRequest::IsActive,
        Action::Name => StartRequest::Name,
        Action::Start => StartRequest::Api,
        Action::ExitCode => StartRequest::ExitCode,
        Action::UserData => StartRequest::UserData,
        Action::Welcome => StartRequest::Welcome,
    };
    request_sender.try_send(start_request)?;
    let response = timeout(Duration::from_secs(2), response_receiver.recv())
        .await?
        .result_reason("No response")?;

    Ok(response)
}

async fn handle_kuma_request(
    State(data): State<ServerConfig>,
    Path((action, user_name)): Path<(String, String)>,
) -> impl IntoResponse {
    match handle_kuma(&data.database, data.map, user_name, action).await {
        Ok(_) => (StatusCode::OK, "OK".to_string()),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

async fn handle_kuma(
    db: &DatabaseConnection,
    instance_map: Arc<RwLock<InstanceMap>>,
    user_name: String,
    action: String,
) -> GenResult<()> {
    let instance_map = &*instance_map.read().await;
    let mut users_to_remove = vec![];
    let mut users_to_add = vec![];
    if action == "delete" {
        if user_name == "all" {
            users_to_remove = instance_map.keys().cloned().collect();
        } else {
            users_to_remove.push(user_name.clone());
        }
    } else if action == "reset" {
        if user_name == "all" {
            users_to_add = instance_map.keys().cloned().collect();
            users_to_remove = instance_map.keys().cloned().collect();
        } else {
            users_to_add.push(user_name.clone());
            users_to_remove.push(user_name);
        }
    } else {
        return Err("Unknown action".into());
    }
    let general_properties = GeneralProperties::load_default_preferences(db).await?;
    kuma::manage_users(
        &users_to_add,
        &users_to_remove,
        instance_map,
        &general_properties,
    )
    .await?;
    Ok(())
}
