use std::sync::Arc;
use std::time::Duration;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router};
use axum::routing::{get, Route};
use tokio::sync::mpsc::Receiver;
use tokio::sync::RwLock;
use tokio::time::timeout;
use crate::execution::StartRequest;
use crate::health::ApplicationLogbook;
use crate::variables::UserInstanceData;
use crate::watchdog::InstanceMap;

#[derive(Clone)]
pub struct ServerConfig {
    map: Arc<RwLock<InstanceMap>>,
}

pub async fn api(instance_map: Arc<RwLock<InstanceMap>>) {
    let config = ServerConfig {
        map: instance_map.clone()
    };
    let app = Router::new()
        .route("/logbook/{user_name}", get(return_logbook))
        .route("/start/{user_name}", get(start_instance))
        .route("/name/{user_name}", get(get_name))
        .with_state(config);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn return_logbook(State(data): State<ServerConfig>, Path(user_name): Path<String>) -> impl IntoResponse {
    info!("Got a request for {user_name}");
    match data.map.read().await.get(&user_name) {
        Some(instance) => {
            instance.request_sender.try_send(StartRequest::Logbook);
            let response = timeout(Duration::from_secs(2), instance.response_reciever.write().await.recv()).await;
            (StatusCode::OK, Json(response.unwrap().unwrap().logbook.unwrap())).into_response()
        },
        None => (StatusCode::IM_A_TEAPOT, "User not found".to_string()).into_response(),
    }
}

async fn start_instance(State(data): State<ServerConfig>, Path(user_name): Path<String>) -> impl IntoResponse {
    info!("Got a request for {user_name}");
    match data.map.read().await.get(&user_name) {
        Some(instance) => {
            instance.request_sender.try_send(StartRequest::Pipe);
            let response = timeout(Duration::from_secs(2), instance.response_reciever.write().await.recv()).await;
            (StatusCode::OK, Json(response.unwrap().unwrap().started.unwrap())).into_response()
        },
        None => (StatusCode::IM_A_TEAPOT, "User not found".to_string()).into_response(),
    }
}

async fn get_name(State(data): State<ServerConfig>, Path(user_name): Path<String>) -> impl IntoResponse {
    info!("Got a request for {user_name}");
    match data.map.read().await.get(&user_name) {
        Some(instance) => {
            instance.request_sender.try_send(StartRequest::Name);
            let response = timeout(Duration::from_secs(2), instance.response_reciever.write().await.recv()).await;
            (StatusCode::OK, Json(response.unwrap().unwrap().name.unwrap())).into_response()
        },
        None => (StatusCode::IM_A_TEAPOT, "User not found".to_string()).into_response(),
    }
}