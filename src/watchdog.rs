use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use dotenvy::var;
use sea_orm::DatabaseConnection;
use tokio::{
    sync::{
        RwLock,
        mpsc::{Sender, channel},
    },
    task::JoinHandle,
};

use crate::{
    GenResult,
    execution::StartReason,
    main_loop,
    variables::{GeneralProperties, ThreadShare, UserData, UserInstanceData},
};

#[derive(Debug, PartialEq)]
enum InstanceState {
    New,
    Remain,
    Remove,
}

pub struct UserInstance {
    pub user_instance_data: UserInstanceData,
    pub thread_handle: JoinHandle<()>,
    pub sender: Sender<StartReason>,
}

impl UserInstance {
    pub async fn new(user_data: UserInstanceData) -> Self {
        let channel = channel(1);
        let thread = tokio::spawn(main_loop(channel.1, user_data.clone()));
        Self {
            user_instance_data: user_data,
            thread_handle: thread,
            sender: channel.0,
        }
    }
}

type InstanceName = String;

type InstanceMap = HashMap<InstanceName, UserInstance>;

type RwCell<T> = LazyLock<RwLock<Option<T>>>;

static DEFAULT_PROPERTIES: RwCell<ThreadShare<GeneralProperties>> =
    LazyLock::new(|| RwLock::new(None));

pub async fn watchdog(db: &DatabaseConnection) -> GenResult<()> {
    let mut instances: InstanceMap = HashMap::new();
    let users = UserData::get_all_usernames(db).await?;
    start_stop_instances(db, &mut instances, &users).await?;
    info!("Users: {users:#?}");
    instances
        .get(users.first().unwrap())
        .unwrap()
        .sender
        .send(StartReason::Timer)
        .await;
    loop {}
    Ok(())
}

async fn start_stop_instances(
    db: &DatabaseConnection,
    active_instances: &mut InstanceMap,
    db_users: &Vec<String>,
) -> GenResult<()> {
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
    if let Some(default_properties) = DEFAULT_PROPERTIES.write().await.clone() {
        *default_properties.write().await = GeneralProperties::load_default_preferences(db).await?;
    } else {
        let default_preferences = GeneralProperties::load_default_preferences(db).await?;
        DEFAULT_PROPERTIES
            .write()
            .await
            .replace(Arc::new(RwLock::new(default_preferences)));
    }
    let instances_to_remove =
        get_equal_instances(InstanceState::Remove, &instances_state, active_instances);
    let instances_to_refresh =
        get_equal_instances(InstanceState::Remain, &instances_state, active_instances);
    let instances_to_add =
        get_equal_instances(InstanceState::New, &instances_state, active_instances);

    add_instances(db, &instances_to_add, active_instances).await?;
    stop_instances(&instances_to_remove, active_instances);
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

async fn add_instances(
    db: &DatabaseConnection,
    instances_to_add: &Vec<String>,
    active_instances: &mut InstanceMap,
) -> GenResult<()> {
    // Load the default preferences, load that to the static variable and then also return the value.
    let default_preferences = DEFAULT_PROPERTIES
        .read()
        .await
        .clone()
        .expect("Default preferences not set");

    for new_user in instances_to_add {
        let user_data_option =
            UserInstanceData::load_user(db, &new_user, default_preferences.clone()).await?;
        if let Some(user_data) = user_data_option {
            info!("Starting user {new_user}");
            let new_instance = UserInstance::new(user_data).await;
            active_instances.insert(new_user.clone(), new_instance);
        } else {
            info!("Failed to add user {new_user}, no entry was found");
        }
    }
    Ok(())
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
