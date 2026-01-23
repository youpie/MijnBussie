use std::sync::Arc;

use chrono::Duration;
use entity::{user_data, user_properties};
use sea_orm::{ActiveValue::Set, EntityTrait, IntoActiveModel};
use serde::Serialize;
use tokio::sync::RwLock;
use tracing::*;

const AUTO_DELETE_DURATION: Duration = Duration::days(31);
const FRESH_DELETE_DURATION: Duration = Duration::days(1);

use crate::{
    GenResult, create_path,
    database::variables::UserData,
    errors::{FailureType, OptionResult, ResultLog, SignInFailure},
    get_data, get_database_connection,
    webcom::email::{DeletedReason, send_account_deleted_mail, send_deletion_warning_mail},
};

// The current system is really messy if you want to update user values from the database,
// Because you need to write to the instance data from the instance itself, which is not really what I want
// There should be a single function to call to update values of the instance to the database and the application local
pub async fn update_instance_timestamps(
    exit_code: &FailureType,
    instance_data: Arc<RwLock<UserData>>,
    execution_by_system: bool,
) -> GenResult<()> {
    let db = get_database_connection().await;
    let (user, _properties) = get_data();
    let user_data = user_data::Entity::find_by_id(user.id).one(&db).await;
    if let Ok(Some(user)) = user_data {
        let mut active_user = user.into_active_model();
        let timestamp = chrono::offset::Utc::now().naive_utc();
        let mut instance_data = instance_data.write().await;
        active_user.last_execution_date = Set(Some(timestamp.clone()));
        instance_data.last_execution_date = Some(timestamp.clone());
        if exit_code != &FailureType::SignInFailed(SignInFailure::IncorrectCredentials) {
            active_user.last_succesfull_sign_in_date = Set(Some(timestamp.clone()));
            instance_data.last_succesfull_sign_in_date = Some(timestamp.clone());
        }
        if execution_by_system {
            active_user.last_system_execution_date = Set(Some(timestamp.clone()));
            instance_data.last_system_execution_date = Some(timestamp.clone());
        }
        user_data::Entity::update(active_user)
            .validate()?
            .exec(&db)
            .await?;
    }
    Ok(())
}

#[derive(Debug, Serialize, Clone)]
enum InstanceStanding {
    Safe,
    Fresh,
    InDanger,
    AlmostDeleted,
    MustDelete,
    MustDeleteFresh,
}

#[derive(Debug, Serialize, Clone)]
pub struct StandingInformation {
    standing: InstanceStanding,
    failed_days: Option<i64>,
    deletion_threshold: i64,
    warning_sent: bool,
}

impl StandingInformation {
    pub fn get() -> Self {
        let (user, _properties) = get_data();
        let current_time = chrono::offset::Utc::now().naive_utc();
        let standing = InstanceStanding::get_standing();
        let failed_days = user
            .last_succesfull_sign_in_date
            .clone()
            .and_then(|date| Some(current_time.signed_duration_since(date).num_days()));
        let deletion_threshold = AUTO_DELETE_DURATION.num_days();
        let warning_sent = create_path("warning_sent").exists();
        Self {
            standing,
            failed_days,
            deletion_threshold,
            warning_sent,
        }
    }
}

impl InstanceStanding {
    fn get_standing() -> InstanceStanding {
        let (user, _properties) = get_data();

        if !user.user_properties.auto_delete_account {
            return InstanceStanding::Safe;
        }

        let current_time = chrono::offset::Utc::now().naive_utc();
        match user.last_succesfull_sign_in_date.clone() {
            Some(sign_in_date)
                if sign_in_date.eq(&user.last_execution_date.unwrap_or_default()) =>
            {
                Self::Safe
            }
            Some(sign_in_date)
                if current_time.signed_duration_since(sign_in_date) >= AUTO_DELETE_DURATION =>
            {
                Self::MustDelete
            }
            Some(sign_in_date)
                if current_time.signed_duration_since(sign_in_date)
                    >= AUTO_DELETE_DURATION - Duration::days(7) =>
            {
                Self::AlmostDeleted
            }
            None if current_time.signed_duration_since(user.creation_date)
                >= FRESH_DELETE_DURATION =>
            {
                Self::MustDeleteFresh
            }
            None => Self::Fresh,
            _ => Self::InDanger,
        }
    }
}

// If true, kill current instance
pub async fn check_instance_standing() -> bool {
    let (user, _properties) = get_data();
    let warning_sent_path = create_path("warning_sent");

    match InstanceStanding::get_standing() {
        InstanceStanding::Safe if warning_sent_path.exists() => {
            std::fs::remove_file(warning_sent_path).warn("Removing warning sent file");
        }
        InstanceStanding::AlmostDeleted => {
            send_deletion_warning_mail().warn("sending deletion warning");
            std::fs::write(warning_sent_path, []).warn("writing deletion sent warning");
        }
        InstanceStanding::MustDelete => {
            delete_account(user.id, DeletedReason::OldAge)
                .await
                .warn("Removing user");
            return true;
        }
        InstanceStanding::MustDeleteFresh => {
            delete_account(user.id, DeletedReason::NewDead)
                .await
                .warn("Removing fresh user");
            return true;
        }
        _ => (),
    };
    false
}

pub async fn delete_account(user_id: i32, reason: DeletedReason) -> GenResult<()> {
    let db = get_database_connection().await;
    let path = create_path("");
    warn!("Deleting user");
    info!("{path:?}");
    std::fs::remove_dir_all(path).warn("Deleting user dir");
    let user_data = UserData::get_id(&db, user_id).await?.result()?;
    let properties_id = user_data.user_properties.user_properties_id;
    user_data::Entity::delete_by_id(user_id)
        .exec(&db)
        .await
        .warn("Removing user data");
    user_properties::Entity::delete_by_id(properties_id)
        .exec(&db)
        .await
        .warn("Removing user properties");
    send_account_deleted_mail(reason).warn("Sending deletion mail");
    Ok(())
}
