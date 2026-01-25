use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::{
    GENERAL_PROPERTIES, GenResult, NAME, StartRequest, USER_PROPERTIES,
    database::variables::{GeneralProperties, ThreadShare, UserData, UserInstanceData},
    execution::timer::{calculate_initial_execution_time, get_system_time},
    kuma, user_instance,
};
use crate::{errors::FailureType, kuma::KumaUserRequest};
use crate::{errors::ResultLog, kuma::KumaAction};
use crate::{health::ApplicationLogbook, webcom::deletion::StandingInformation};
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
use tracing_futures::Instrument;

#[derive(Debug, PartialEq)]
enum InstanceState {
    New,
    Remain,
    Remove,
}

#[derive(Clone, PartialEq, Debug)]
pub enum WatchdogRequest {
    SingleUser(String),
    KumaRequest((KumaAction, KumaUserRequest)),
    AllUser,
    FirstTime,
}

#[derive(Debug, Clone, Serialize)]
pub enum RequestResponse {
    Logbook(ApplicationLogbook),
    Name(String),
    Active(bool),
    ExitCode(FailureType),
    UserData(UserData),
    GenResponse(String),
    InstanceStanding(StandingInformation),
}

pub struct UserInstance {
    pub user_instance_data: UserInstanceData,
    pub thread_handle: JoinHandle<()>,
    pub request_sender: Arc<Sender<StartRequest>>,
    pub response_receiver: RwLock<Receiver<RequestResponse>>,
    pub execution_time: Time,
}

impl UserInstance {
    pub async fn new(user_data: UserInstanceData) -> Self {
        let user_name = user_data.user_data.read().await.user_name.clone();
        let span = warn_span!("Instance", user_name);
        let request_channel = channel(1);
        let request_sender_arc = Arc::new(request_channel.0);
        let response_channel = channel(1);
        let data_clone = user_data.clone();
        let thread = tokio::spawn(
            USER_PROPERTIES.scope(
                RefCell::new(None),
                GENERAL_PROPERTIES.scope(
                    RefCell::new(None),
                    NAME.scope(
                        RefCell::new(None),
                        user_instance(
                            request_channel.1,
                            response_channel.0,
                            request_sender_arc.clone(),
                            data_clone,
                        )
                        .instrument(span),
                    ),
                ),
            ),
        );

        let user_data_clone = user_data.user_data.read().await.clone();
        let execution_time = calculate_initial_execution_time(
            user_data_clone.last_system_execution_date,
            user_data_clone.user_properties.execution_interval_minutes,
            user_data_clone.user_properties.execution_minute,
        )
        .await;

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
            request_sender: request_sender_arc,
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
        debug!("{channel_wait:?}");
        if let Ok(Some(ref request)) = channel_wait
            && let WatchdogRequest::SingleUser(user) = request
        {
            info!("Updating user because of request {user}");
            update_individual_user(
                db,
                vec![user.clone()],
                &mut *instances.clone().write().await,
            )
            .await
            .warn("deleting individual user");
        } else if let Ok(Some(WatchdogRequest::KumaRequest(ref request))) = channel_wait {
            let general_properties = GeneralProperties::load_default_preferences(db).await?;
            kuma::manage_users(
                vec![request.clone()],
                &*instances.read().await,
                &general_properties,
            )
            .await
            .warn("Api kuma run");
        } else if channel_wait == Ok(None) {
            return Err("Notification channel closed".into());
        } else {
            debug!("Updating users");
            let users = UserData::get_all_usernames(db).await?;
            start_stop_instances(
                db,
                instances.clone(),
                &users,
                channel_wait.eq(&Ok(Some(WatchdogRequest::FirstTime))),
            )
            .await?;
            debug!("Users: {users:#?}");
        }
    }
}

async fn start_stop_instances(
    db: &DatabaseConnection,
    active_instances: Arc<RwLock<InstanceMap>>,
    db_users: &Vec<String>,
    first_run: bool,
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
    let default_preferences = get_default_preferences(db).await?;
    // If the preferences are already set, only replace the value inside the RwLock

    let instances_to_remove =
        get_equal_instances(InstanceState::Remove, &instances_state, &active_instances);
    let instances_to_refresh =
        get_equal_instances(InstanceState::Remain, &instances_state, &active_instances);
    let instances_to_add =
        get_equal_instances(InstanceState::New, &instances_state, &active_instances);
    add_instances(db, &instances_to_add, &mut active_instances).await;
    refresh_instances(db, &instances_to_refresh, &mut active_instances).await;
    if !first_run {
        kuma::manage_users(
            vec![
                (
                    KumaAction::Delete,
                    KumaUserRequest::Users(instances_to_remove.clone()),
                ),
                (KumaAction::Add, KumaUserRequest::Users(instances_to_add)),
            ],
            &active_instances,
            &default_preferences,
        )
        .await
        .warn("Kuma run");
    } else {
        debug!("Skipped kuma due to first run");
    }
    stop_instances(&instances_to_remove, &mut active_instances);
    Ok(())
}

async fn get_default_preferences(db: &DatabaseConnection) -> GenResult<GeneralProperties> {
    if let Some(default_properties) = DEFAULT_PROPERTIES.write().await.clone() {
        let default_preferences = GeneralProperties::load_default_preferences(db).await?;
        *default_properties.write().await = default_preferences.clone();
        Ok(default_preferences)
    // If the preferences are not yet set, create a new Arc and RwLock
    } else {
        let default_preferences = GeneralProperties::load_default_preferences(db).await?;
        DEFAULT_PROPERTIES
            .write()
            .await
            .replace(Arc::new(RwLock::new(default_preferences.clone())));
        Ok(default_preferences)
    }
}

async fn update_individual_user(
    db: &DatabaseConnection,
    user_names: Vec<String>,
    active_instances: &mut InstanceMap,
) -> GenResult<()> {
    let mut instances_to_add = vec![];
    let mut instances_to_refresh = vec![];
    let mut instances_to_remove = vec![];

    let default_preferences = get_default_preferences(db).await?;

    for user in user_names {
        if !active_instances.contains_key(&user) {
            instances_to_add.push(user);
        } else if UserData::get_from_username(db, &user).await?.is_none() {
            instances_to_remove.push(user);
        } else {
            instances_to_refresh.push(user);
        }
    }
    add_instances(db, &instances_to_add, active_instances).await;
    refresh_instances(db, &instances_to_refresh, active_instances).await;
    kuma::manage_users(
        vec![
            (
                KumaAction::Delete,
                KumaUserRequest::Users(instances_to_remove.clone()),
            ),
            (KumaAction::Add, KumaUserRequest::Users(instances_to_add)),
        ],
        active_instances,
        &default_preferences,
    )
    .await
    .warn("Kuma run individual");
    stop_instances(&instances_to_remove, active_instances);
    Ok(())
}

fn stop_instances(instances_to_stop: &Vec<String>, active_instances: &mut InstanceMap) {
    for instance_name in instances_to_stop {
        if let Some(instance) = active_instances.get(instance_name) {
            instance.thread_handle.abort_handle().abort();
        }
        warn!("Deleting instance: {instance_name}");
        active_instances.remove(instance_name);
    }
}

async fn refresh_instances(
    db: &DatabaseConnection,
    instances_to_refresh: &Vec<String>,
    active_instances: &mut InstanceMap,
) {
    let mut instances_to_add = vec![];
    for insance_name in instances_to_refresh {
        if let Some(instance) = active_instances.get_mut(insance_name) {
            instance
                .user_instance_data
                .update_user(db)
                .await
                .warn("Updating User");
        } else {
            instances_to_add.push(insance_name.clone());
        }
    }
    if !instances_to_add.is_empty() {
        add_instances(db, &instances_to_add, active_instances).await;
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
