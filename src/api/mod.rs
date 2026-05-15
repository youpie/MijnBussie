use std::{path::PathBuf, str::FromStr, sync::Arc};

use axum::{Router, middleware, routing::get};
use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::{RwLock, mpsc::Sender};

use self::auth::check_api_key;
use self::route::*;
use crate::{execution::watchdog::WatchdogRequest, instance::InstanceMap};

mod auth;
pub mod route;

#[derive(Clone)]
pub struct ServerConfig {
    map: Arc<RwLock<InstanceMap>>,
    sender: Sender<WatchdogRequest>,
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
        .route("/{user_name}/delete", get(remove_instance))
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
