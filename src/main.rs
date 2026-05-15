const MAIN_URL: &str = "webcom.connexxion.nl";
// the ;x should be equal to the ammount of fallback URLs
const FALLBACK_URL: [&str; 2] = [
    "https://dmz-wbc-web01.connexxion.nl/WebComm/default.aspx",
    "https://dmz-wbc-web02.connexxion.nl/WebComm/default.aspx",
];
const APPLICATION_NAME: &str = "Mijn Bussie";

use crate::execution::watchdog::WatchdogRequest;
use crate::execution::*;
use crate::instance::InstanceMap;
use crate::prelude::*;
use dotenvy::dotenv_override;
use migration::Migrator;
use migration::MigratorTrait;
use rustls::crypto::CryptoProvider;
use rustls::crypto::ring::default_provider;
use std::collections::HashMap;
use std::fs::set_permissions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::mpsc::channel;
use tracing::level_filters::LevelFilter;
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
mod instance;
mod kuma;
mod prelude;

pub fn create_path_local(
    user: &instance::UserData,
    properties: &instance::GeneralProperties,
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

pub fn set_strict_file_permissions(path: &PathBuf) -> GenResult<()> {
    let file = std::fs::File::open(&path)?;
    let metadata = file.metadata()?;
    let mut file_mode = metadata.permissions();
    file_mode.set_mode(0o100600);
    set_permissions(&path, file_mode)?;
    Ok(())
}

#[allow(dead_code)]
fn check_env_permissions() -> GenResult<()> {
    let uid = std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .warn_owned("Failed to get uid")
        .ok();
    let permissions_target = 0o100600;
    let metadata = std::fs::File::open("./.env")?.metadata()?;
    let file_mode = metadata.permissions().mode();
    let file_owner = metadata.uid();
    if file_mode == permissions_target && Some(file_owner) == uid {
        Ok(())
    } else {
        Err(anyhow!(
            "INCORRECT PERMISSIONS FOR ENV. Should be {permissions_target:o}, is {file_mode:o}. File owner should be {uid:?}, is {file_owner}"
        )
        .into())
    }
}

#[tokio::main]
async fn main() -> GenResult<()> {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()
        .unwrap();

    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_filter(filter);

    let global_subscriber = Registry::default().with(stdout_layer);
    tracing::subscriber::set_global_default(global_subscriber)
        .expect("Failed to set global subscriber");
    #[cfg(not(debug_assertions))]
    {
        check_env_permissions().unwrap();
    }

    dotenv_override().expect("Failed to read ENV file");
    info!("Starting {APPLICATION_NAME}");
    CryptoProvider::install_default(default_provider()).unwrap();

    let db = database::get_database_connection().await;

    // Apply all pending migrations
    Migrator::up(&db, None)
        .await
        .expect("Failed to apply Database changes");

    let (watchdog_tx, mut watchdog_rx) = channel(2);
    _ = watchdog_tx.try_send(WatchdogRequest::FirstTime);

    let instances: Arc<RwLock<InstanceMap>> = Arc::new(RwLock::new(HashMap::new()));

    tokio::spawn(timer::execution_timer(instances.clone()));
    tokio::spawn(api::api(instances.clone(), watchdog_tx));

    watchdog::watchdog(instances.clone(), &db, &mut watchdog_rx)
        .await
        .expect("Watchdog error");

    info!("Stopping {APPLICATION_NAME}");
    Ok(())
}
