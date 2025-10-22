use std::sync::Arc;

use serde::Serialize;
use time::{Duration, OffsetDateTime, Time};
use tokio::{sync::RwLock, time::sleep};

use crate::{GenResult, email::TIME_DESCRIPTION, variables::UserData, watchdog::InstanceMap};

#[allow(dead_code)]
#[derive(PartialEq, Serialize)]
pub enum StartReason {
    Direct,
    Timer,
    Single,
    Pipe,
    Force,
}

pub fn get_system_time() -> Time {
    OffsetDateTime::now_local()
        .unwrap_or(OffsetDateTime::now_utc())
        .time()
}

#[allow(dead_code, unused_variables)]
pub async fn calculate_next_execution_time(data: Arc<RwLock<UserData>>, first_time: bool) -> Time {
    let mut current_system_time = get_system_time();
    if let Ok(zerod_system_time) = current_system_time.replace_second(0) {
        current_system_time = zerod_system_time;
    }
    let user_properties = &data.read().await.user_properties;
    let interval = user_properties.execution_interval_minutes;

    let execution_time = if first_time {
        let execution_minute = user_properties.execution_minute;
        let random_execution_hour = rand::random_range(0..=interval / 3600);

        let mut execution_time =
            current_system_time + Duration::hours(random_execution_hour.into());

        if let Ok(adjusted_start) = execution_time.replace_minute(execution_minute as u8) {
            execution_time = adjusted_start
        }
        execution_time
    } else {
        current_system_time + Duration::minutes(interval.into())
    };

    execution_time
}

pub async fn execution_timer(instances: Arc<RwLock<InstanceMap>>) -> GenResult<()> {
    loop {
        let sleep_time = 60 - get_system_time().second() as u64 + 1;
        debug!("timer sleeping for {sleep_time} seconds");
        sleep(std::time::Duration::from_secs(sleep_time)).await;
        let instances = &mut *instances.write().await;
        let current_system_time = get_system_time();
        for instance in instances {
            if instance.1.execution_time <= current_system_time {
                let user_name = instance.0;
                debug!(
                    "Executing user {} at {}",
                    user_name,
                    current_system_time.format(TIME_DESCRIPTION).unwrap()
                );
                _ = instance.1.sender.try_send(StartReason::Timer);
                instance.1.execution_time = calculate_next_execution_time(
                    instance.1.user_instance_data.user_data.clone(),
                    false,
                )
                .await;
            }
        }
    }
}
