use std::collections::HashMap;

use sea_orm::DatabaseConnection;
use tokio::task::JoinHandle;

use crate::{
    GenResult,
    variables::{UserInstanceData, UserData},
};

#[derive(Debug, PartialEq)]
enum InstanceState {
    New,
    Remain,
    Remove,
}

pub struct UserInstance {
    pub user_instance_data: UserInstanceData,
    pub thread_handle: Option<JoinHandle<GenResult<()>>>,
}

type InstanceName = String;

type InstanceMap = HashMap<InstanceName, UserInstance>;

pub async fn watchdog(db: &DatabaseConnection) -> GenResult<()> {
    let instances: InstanceMap = HashMap::new();
    let users = UserData::get_all_usernames(db).await?;
    info!("Users: {users:#?}");
    Ok(())
}

async fn start_stop_instances(
    db: &DatabaseConnection,
    active_instances: &mut InstanceMap,
    db_users: Vec<String>,
) -> GenResult<()> {
    let mut instances_state: HashMap<InstanceName, InstanceState> = HashMap::new();
    for active_instance in &mut *active_instances {
        instances_state.insert(active_instance.0.to_owned(), InstanceState::Remove);
    }

    for db_user in db_users {
        match instances_state.get_mut(&db_user) {
            Some(instance) => *instance = InstanceState::Remain,
            None => {
                instances_state.insert(db_user, InstanceState::New);
            }
        };
    }

    let instances_to_remove =
        get_equal_instances(InstanceState::Remove, &instances_state, active_instances);
    let instances_to_refresh =
        get_equal_instances(InstanceState::Remain, &instances_state, active_instances);
    let instances_to_add = get_equal_instances(InstanceState::Remove, &instances_state, active_instances);
    stop_instances(&instances_to_remove, active_instances);
    Ok(())
}

fn stop_instances(instances_to_stop: &Vec<String>, active_instances: &mut InstanceMap) {
    for instance_name in instances_to_stop {
        if let Some(instance) = active_instances.get(instance_name)
            && let Some(handle) = &instance.thread_handle
        {
            handle.abort_handle().abort();
        }
        active_instances.remove(instance_name);
    }
}

fn add_instances(instances_to_add: &Vec<String>, active_instances: &mut InstanceMap) {}

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
