use serde::Serialize;
use std::{cell::RefCell, collections::HashMap, sync::Arc};
use time::{Time, macros::format_description};
use tokio::{
    sync::{
        RwLock,
        mpsc::{Receiver, Sender, channel},
    },
    task::JoinHandle,
    task_local,
};
use tracing_futures::Instrument;

use crate::errors::FailureType;
use crate::execution::timer::{calculate_initial_execution_time, get_system_time};
use crate::health::ApplicationLogbook;

pub use self::data::{get_data, set_data};
pub use self::shift::*;
pub use crate::database::{get_database_connection, variables::*};
pub use crate::prelude::*;

pub const TIME_DESCRIPTION: &[time::format_description::BorrowedFormatItem<'_>] =
    format_description!("[hour]:[minute]");
pub const DATE_DESCRIPTION: &[time::format_description::BorrowedFormatItem<'_>] =
    format_description!("[day]-[month]-[year]");

pub mod data;
mod deletion;
pub mod email;
mod gebroken_shifts;
pub mod ical;
mod parsing;
pub mod shift;
mod user_instance;
pub mod webcom;
mod webdriver;

task_local! {
    static NAME: RefCell<Option<String>>;
    static USER_PROPERTIES: RefCell<Option<Arc<UserData>>>;
    static GENERAL_PROPERTIES: RefCell<Option<Arc<GeneralProperties>>>;
}

pub type InstanceName = String;

pub type InstanceMap = HashMap<InstanceName, UserInstance>;

#[derive(Debug, Clone, Serialize)]
pub enum RequestResponse {
    Logbook(ApplicationLogbook),
    Name(String),
    Active(ActiveState),
    Started(Started),
    ExitCode(FailureType),
    UserData(UserData),
    GenResponse(String),
    InstanceStanding(deletion::StandingInformation),
}

#[derive(Debug, Clone, Serialize)]
pub enum ActiveState {
    Active,
    SignedIn,
    Dead,
}

#[derive(Debug, Clone, Serialize)]
pub enum Started {
    Started,
    AlreadyActive,
}

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
    Logs,

    // Webcom request
    ExecutionFinished(FailureType),
    SignedIn,
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
                        user_instance::user_instance(
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
