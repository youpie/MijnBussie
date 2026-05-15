use crate::GenResult;
use crate::execution::watchdog::WatchdogRequest;
use crate::instance::{RequestResponse, StartRequest};
use crate::kuma::{KumaAction, KumaUserRequest};
use anyhow::Context;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use strum_macros::EnumString;
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

use super::*;
use crate::prelude::*;

#[derive(Clone, EnumString, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "snake_case"))]
pub enum Action {
    Logbook,
    IsActive,
    Name,
    Start,
    ExitCode,
    UserData,
    Welcome,
    Calendar,
    Standing,
    Logs,
}

pub async fn remove_instance(
    State(data): State<ServerConfig>,
    Path(user_name): Path<String>,
) -> impl IntoResponse {
    match data.sender.send(WatchdogRequest::Delete(user_name)).await {
        Ok(_) => (StatusCode::OK, "".to_owned()),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
    .into_response()
}

pub async fn refresh_users(
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

pub async fn get_information(
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
            .warn_owned("Sending request")
            {
                Ok(response) => (StatusCode::OK, Json(response)).into_response(),
                Err(err) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(err.to_string())).into_response()
                }
            }
        }
        None => (StatusCode::NO_CONTENT, Json("User not found".to_string())).into_response(),
    }
}

pub async fn send_request(
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
        Action::Standing => StartRequest::Standing,
        Action::Logs => StartRequest::Logs,
    };
    // if start_request == StartRequest::Delete {
    //     debug!("Deletion request to watchdog");
    //     watchdog_sender
    //         .send(WatchdogRequest::SingleUser(user_name))
    //         .await
    //         .warn("Sending delete refresh to watchdog");
    // }
    debug!("Got network request for {start_request:?}");
    request_sender.try_send(start_request).context("Send")?;
    let response = timeout(Duration::from_secs(10), response_receiver.recv())
        .await?
        .result_reason("No response")
        .context("Recieve")?;

    Ok(response)
}

pub async fn handle_kuma_request(
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
