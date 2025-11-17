use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::health::ApplicationLogbook;
use crate::{
    GENERAL_PROPERTIES, GenResult, NAME, USER_PROPERTIES,
    database::variables::{GeneralProperties, ThreadShare, UserData, UserInstanceData},
    execution::timer::{StartRequest, calculate_next_execution_time, get_system_time},
    kuma, user_instance,
};
use crate::{errors::FailureType, kuma::KumaUserRequest};
use crate::{errors::ResultLog, kuma::KumaAction};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use time::Time;
use tokio::{
    sync::{
        RwLock,
        mpsc::{Receiver, Sender, channel},
    },
    task::JoinHandle,
    time::timeout,
};
use tracing::*;

#[derive(Debug, PartialEq)]
enum InstanceState {
    New,
    Remain,
    Remove,
}

#[derive(Clone)]
pub enum WatchdogRequest {
    SingleUser(String),
    KumaRequest((KumaAction, KumaUserRequest)),
    AllUser,
}

#[derive(Debug, Clone, Serialize)]
pub enum RequestResponse {
    Logbook(ApplicationLogbook),
    Name(String),
    Active(bool),
    ExitCode(FailureType),
    UserData(UserData),
    GenResponse(String),
}

pub struct UserInstance {
    pub user_instance_data: UserInstanceData,
    pub thread_handle: JoinHandle<()>,
    pub request_sender: Sender<StartRequest>,
    pub response_receiver: RwLock<Receiver<RequestResponse>>,
    pub execution_time: Time,
}

impl UserInstance {
    pub async fn new(user_data: UserInstanceData) -> Self {
        let request_channel = channel(1);
        let response_channel = channel(1);
        let data_clone = user_data.clone();
        let thread = tokio::spawn(USER_PROPERTIES.scope(
            RefCell::new(None),
            GENERAL_PROPERTIES.scope(
                RefCell::new(None),
                NAME.scope(
                    RefCell::new(None),
                    user_instance(request_channel.1, response_channel.0, data_clone),
                ),
            ),
        ));
        let execution_time = calculate_next_execution_time(user_data.user_data.clone(), true).await;
        info!(
            "Executing user {} in {} minutes",
            user_data.user_data.read().await.user_name,
            get_system_time()
                .duration_until(execution_time)
                .whole_minutes()
        );
        Self {
            user_instance_data: user_data,
            thread_handle: thread,
            request_sender: request_channel.0,
            response_receiver: RwLock::new(response_channel.1),
            execution_time,
        }
    }
}

type InstanceName = String;

pub type InstanceMap = HashMap<InstanceName, UserInstance>;

type RwCell<T> = LazyLock<RwLock<Option<T>>>;

static DEFAULT_PROPERTIES: RwCell<ThreadShare<GeneralProperties>> =
    LazyLock::new(|| RwLock::new(None));

pub async fn watchdog(
    instances: Arc<RwLock<InstanceMap>>,
    db: &DatabaseConnection,
    receiver: &mut Receiver<WatchdogRequest>,
) -> GenResult<()> {
    loop {
        // Update all users in the database every 30 minutes
        let channel_wait = timeout(Duration::from_secs(60 * 30), receiver.recv()).await;
        if let Ok(Some(ref request)) = channel_wait
            && let WatchdogRequest::SingleUser(user) = request
        {
            info!("Updating user because of request {user}");
            refresh_instances(db, &vec![user.clone()], &mut *instances.write().await).await;
        } else if let Ok(Some(WatchdogRequest::KumaRequest(ref request))) = channel_wait {
            let general_properties = GeneralProperties::load_default_preferences(db).await?;
            kuma::manage_users(
                vec![request.clone()],
                &*instances.read().await,
                &general_properties,
            )
            .await
            .warn("Api kuma run");
        } else {
            debug!("Updating users");
            let users = UserData::get_all_usernames(db).await?;
            start_stop_instances(db, instances.clone(), &users).await?;
            debug!("Users: {users:#?}");
        }
    }
}

async fn start_stop_instances(
    db: &DatabaseConnection,
    active_instances: Arc<RwLock<InstanceMap>>,
    db_users: &Vec<String>,
) -> GenResult<()> {
    let mut active_instances = active_instances.write().await;
    let mut instances_state: HashMap<InstanceName, InstanceState> = HashMap::new();
    for active_instance in &mut *active_instances {
        instances_state.insert(active_instance.0.to_owned(), InstanceState::Remove);
    }

    for db_user in db_users {
        match instances_state.get_mut(db_user) {
            Some(instance) => *instance = InstanceState::Remain,
            None => {
                instances_state.insert(db_user.clone(), InstanceState::New);
            }
        };
    }
    // Load the default preferences and write them to the global variable
    let default_preferences =
        // If the preferences are already set, only replace the value inside the RwLock
        if let Some(default_properties) = DEFAULT_PROPERTIES.write().await.clone() {
            let default_preferences = GeneralProperties::load_default_preferences(db).await?;
            *default_properties.write().await = default_preferences.clone();
            default_preferences
        // If the preferences are not yet set, create a new Arc and RwLock
        } else {
            let default_preferences = GeneralProperties::load_default_preferences(db).await?;
            DEFAULT_PROPERTIES
                .write()
                .await
                .replace(Arc::new(RwLock::new(default_preferences.clone())));
            default_preferences
        };

    let instances_to_remove =
        get_equal_instances(InstanceState::Remove, &instances_state, &active_instances);
    let instances_to_refresh =
        get_equal_instances(InstanceState::Remain, &instances_state, &active_instances);
    let instances_to_add =
        get_equal_instances(InstanceState::New, &instances_state, &active_instances);
    add_instances(db, &instances_to_add, &mut active_instances).await;
    stop_instances(&instances_to_remove, &mut active_instances);
    refresh_instances(db, &instances_to_refresh, &mut active_instances).await;
    kuma::manage_users(
        vec![
            (
                KumaAction::Delete,
                KumaUserRequest::Users(instances_to_remove),
            ),
            (KumaAction::Add, KumaUserRequest::Users(instances_to_add)),
        ],
        &active_instances,
        &default_preferences,
    )
    .await
    .warn("Kuma run");
    Ok(())
}

fn stop_instances(instances_to_stop: &Vec<String>, active_instances: &mut InstanceMap) {
    for instance_name in instances_to_stop {
        if let Some(instance) = active_instances.get(instance_name) {
            instance.thread_handle.abort_handle().abort();
        }
        active_instances.remove(instance_name);
    }
}

async fn refresh_instances(
    db: &DatabaseConnection,
    instances_to_refresh: &Vec<String>,
    active_instances: &mut InstanceMap,
) {
    for insance_name in instances_to_refresh {
        if let Some(instance) = active_instances.get_mut(insance_name) {
            instance
                .user_instance_data
                .update_user(db)
                .await
                .warn("Updating User");
        }
    }
}

async fn add_instances(
    db: &DatabaseConnection,
    instances_to_add: &Vec<String>,
    active_instances: &mut InstanceMap,
) {
    // Load the default preferences, load that to the static variable and then also return the value.
    let default_preferences = DEFAULT_PROPERTIES
        .read()
        .await
        .clone()
        .expect("Default preferences not set");

    for new_user in instances_to_add {
        match UserInstanceData::load_user(db, &new_user, default_preferences.clone())
            .await
            .warn_owned("Adding user")
            .ok()
            .flatten()
        {
            Some(user_data) => {
                info!("Importing user {new_user}");
                let new_instance = UserInstance::new(user_data).await;
                active_instances.insert(new_user.clone(), new_instance);
            }
            None => warn!("Failed to add user {new_user}"),
        };
    }
}

fn get_equal_instances(
    state: InstanceState,
    instances_state: &HashMap<InstanceName, InstanceState>,
    active_instances: &InstanceMap,
) -> Vec<String> {
    instances_state
        .iter()
        .filter_map(|instance| {
            if instance.1 == &state {
                match state {
                    InstanceState::New => Some(instance.0.clone()),
                    _ => active_instances
                        .get_key_value(instance.0)
                        .and_then(|value| Some(value.0.clone())),
                }
            } else {
                None
            }
        })
        .collect()
}
