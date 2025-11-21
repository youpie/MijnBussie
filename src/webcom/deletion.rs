use chrono::Duration;
use entity::{user_data, user_properties};
use sea_orm::{ActiveValue::Set, DatabaseConnection, EntityTrait, IntoActiveModel};
use tracing::*;

const AUTO_DELETE_DURATION: Duration = Duration::days(31);

use crate::{
    GenResult, create_path,
    database::variables::UserData,
    errors::{FailureType, OptionResult, ResultLog, SignInFailure},
    get_data, get_database_connection,
    webcom::email::{self, send_account_deleted_mail},
};

pub async fn update_instance_timestamps(exit_code: &FailureType) -> GenResult<()> {
    let db = get_database_connection().await;
    let (user, _properties) = get_data();
    let user_data = user_data::Entity::find_by_id(user.id).one(&db).await;
    if let Ok(Some(user)) = user_data {
        let mut active_user = user.into_active_model();
        let timestamp = chrono::offset::Utc::now().naive_utc();
        active_user.last_execution_date = Set(Some(timestamp.clone()));
        if exit_code != &FailureType::SignInFailed(SignInFailure::IncorrectCredentials) {
            active_user.last_succesfull_sign_in_date = Set(Some(timestamp));
        }
        user_data::Entity::update(active_user)
            .validate()?
            .exec(&db)
            .await?;
    }
    Ok(())
}

enum AccountStanding {
    Safe,
    Fresh,
    InDanger,
    AlmostDeleted,
}

impl AccountStanding {}

// If true, kill current instance
pub async fn check_instance_standing() -> GenResult<bool> {
    let db = get_database_connection().await;
    let (user, _properties) = get_data();
    let current_time = chrono::offset::Utc::now().naive_utc();
    let warning_sent_path = create_path("warning_sent");

    // If the instance is not in bad standing, delete mail sent file if it exists
    if let Some(last_sign_in) = user.last_succesfull_sign_in_date
        && current_time - last_sign_in < AUTO_DELETE_DURATION - Duration::days(7)
        && !warning_sent_path.exists()
    {
        info!("deleting deletion mail sent file");
        std::fs::remove_file(warning_sent_path).warn("removing deletion mail sent");
    }
    // If last sign in date was too long ago, delete the instance
    else if let Some(last_sign_in) = user.last_succesfull_sign_in_date
        && current_time - last_sign_in > AUTO_DELETE_DURATION
    {
        delete_account_local(&db, user.id)
            .await
            .warn("Deleting user account");
        return Ok(true);
    }
    // If the last succesful sign in date was never, and the instance is older than 1 day, remove the instance
    else if user.last_succesfull_sign_in_date.is_none()
        && current_time - user.creation_date > Duration::days(1)
    {
        warn!("This account has been dead for one day, so has automatically been deleted");
        delete_account_local(&db, user.id)
            .await
            .warn("Deleting dead account");
        return Ok(true);
    }
    // If the instance is in danger of being deleted, warn the user about that
    else if let Some(last_sign_in) = user.last_succesfull_sign_in_date
        && current_time - last_sign_in >= AUTO_DELETE_DURATION - Duration::days(7)
        && !warning_sent_path.exists()
    {
        email::send_deletion_warning_mail().warn("sending deletion warning");
        std::fs::write(warning_sent_path, []).warn("Touching mail send file");
    }
    Ok(false)
}

pub async fn delete_account() -> GenResult<()> {
    let (user, _properties) = get_data();
    let db = get_database_connection().await;
    delete_account_local(&db, user.id).await
}

async fn delete_account_local(db: &DatabaseConnection, user_id: i32) -> GenResult<()> {
    let path = create_path("");
    warn!("Deleting user");
    info!("{path:?}");
    // fs::remove_dir_all(path).warn("Deleting user dir");
    let user_data = UserData::get_id(db, user_id).await?.result()?;
    let properties_id = user_data.user_properties.user_properties_id;
    debug!("{:?}", user_data::Entity::delete_by_id(user_id));
    debug!("{:?}", user_properties::Entity::delete_by_id(properties_id));
    send_account_deleted_mail().warn("Sending deletion mail");
    Ok(())
}
