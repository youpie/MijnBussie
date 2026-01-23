use std::sync::Arc;

use crate::{
    GenResult, database::variables::UserData, errors::FailureType,
    execution::watchdog::InstanceMap, health::ApplicationLogbook,
};
use chrono::NaiveDateTime;
use serde::Serialize;
use time::{Duration, OffsetDateTime, Time};
use tokio::{sync::RwLock, time::sleep};
use tracing::*;

#[allow(dead_code)]
#[derive(PartialEq, Serialize, Clone, Debug)]
pub enum StartRequest {
    Timer,
    Api,
    Single,
    Force,
    Logbook,
    Name,
    IsActive,
    ExitCode,
    UserData,
    Welcome,
    Calendar,
    Delete,
    Standing,

    // Webcom request
    ExecutionFinished(FailureType),
}

pub fn get_system_time() -> Time {
    let time = OffsetDateTime::now_local()
        .unwrap_or(OffsetDateTime::now_utc())
        .time();
    debug!("system time: {:?}", time);
    time
}

fn get_system_time_zero_seconds() -> Time {
    let mut current_system_time = get_system_time();
    if let Ok(zerod_system_time) = current_system_time.replace_second(0) {
        current_system_time = zerod_system_time
    }
    current_system_time
}

fn calculate_first_execution_time_simple(execution_interval: i32, execution_minute: i32) -> Time {
    let current_system_time = get_system_time_zero_seconds();

    let mut interval_hours = execution_interval / 60;
    if interval_hours == 0 {
        interval_hours += 1
    }

    let random_execution_hour = rand::random_range(0..=interval_hours);

    let mut execution_time = current_system_time + Duration::hours(random_execution_hour.into());

    if let Ok(adjusted_start) = execution_time.replace_minute(execution_minute as u8)
        && (current_system_time.minute() < execution_minute as u8 || random_execution_hour != 0)
    {
        execution_time = adjusted_start
    } else if let Ok(adjusted_start) =
        execution_time.replace_minute(current_system_time.minute() + 1)
    {
        execution_time = adjusted_start
    }
    execution_time
}

// Calculate the first execution time based on when the user was last executed before the program was restarted
// If this is within the execution time interval, the time will be restored
// Otherwise a random interval will be chosen
pub async fn calculate_initial_execution_time(
    last_execution_timestamp: Option<NaiveDateTime>,
    execution_interval: i32,
    execution_minute: i32,
) -> Time {
    if last_execution_timestamp.is_none() {
        return calculate_first_execution_time_simple(execution_interval, execution_minute);
    }

    let mut next_execution_time = Time::from_hms(0, 0, 0).unwrap();
    let current_system_time = get_system_time_zero_seconds();
    let elapsed_minutes_since_last_execution = ApplicationLogbook::get_naive_datetime()
        .signed_duration_since(last_execution_timestamp.unwrap())
        .num_minutes();
    debug!("User was last executed {elapsed_minutes_since_last_execution} minutes ago...");

    // If get the interval - elapsed
    // If it is more than 0, execute the user in that ammount of time
    // otherwise fallback
    let time_until_next_execution =
        execution_interval as i64 - elapsed_minutes_since_last_execution;
    debug!("The calculated next execution is in {time_until_next_execution} min");
    if time_until_next_execution > 0 {
        debug!("This is within this users execution interval window of {execution_interval} mins");
        let mut next_execution_time_local =
            current_system_time + Duration::minutes(time_until_next_execution);
        next_execution_time_local = next_execution_time
            .replace_minute(execution_minute as u8)
            .unwrap_or(next_execution_time_local);
        next_execution_time = next_execution_time_local
    } else {
        next_execution_time =
            calculate_first_execution_time_simple(execution_interval, execution_minute);
        debug!("This is within this users execution interval window of {execution_interval} mins");
    }
    debug!(
        "This user will execute in {} mins",
        next_execution_time
            .duration_since(current_system_time)
            .whole_minutes()
    );
    next_execution_time
}

async fn calculate_next_execution_time(data: Arc<RwLock<UserData>>) -> Time {
    let mut current_system_time = get_system_time();
    if let Ok(zerod_system_time) = current_system_time.replace_second(0) {
        current_system_time = zerod_system_time;
    }
    let user_properties = &data.read().await.user_properties;
    let mut interval_hours = user_properties.execution_interval_minutes / 60;
    if interval_hours == 0 {
        interval_hours += 1
    }
    let execution_minute = user_properties.execution_minute;
    _ = user_properties;

    let next_execution_time = current_system_time + Duration::hours(interval_hours.into());
    next_execution_time
        .replace_minute(execution_minute as u8)
        .unwrap_or(next_execution_time)
}

pub async fn execution_timer(instances: Arc<RwLock<InstanceMap>>) -> GenResult<()> {
    let mut first = true;
    loop {
        if !first {
            let sleep_time = 60 - get_system_time().second() as u64 + 1;
            debug!("timer sleeping for {sleep_time} seconds");
            sleep(std::time::Duration::from_secs(sleep_time)).await;
        } else {
            first = false;
        }
        let instances = &mut *instances.write().await;
        let current_system_time = get_system_time();
        let system_time_hm = (current_system_time.hour(), current_system_time.minute());
        for instance in instances.iter_mut() {
            let instance_execution = instance.1.execution_time;
            let instance_time_hm = (instance_execution.hour(), instance_execution.minute());
            if instance_time_hm == system_time_hm {
                let user_name = instance.0;
                debug!("Starting instance {user_name}");
                _ = instance.1.request_sender.try_send(StartRequest::Timer);
                instance.1.execution_time =
                    calculate_next_execution_time(instance.1.user_instance_data.user_data.clone())
                        .await;
                debug!(
                    "Executing user {user_name} at {} next",
                    instance.1.execution_time
                )
            }
        }
    }
}
