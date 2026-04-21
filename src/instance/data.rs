use std::{sync::Arc};

use entity::user_data;
use sea_orm::ActiveValue::Set;
use sea_orm::{EntityTrait, IntoActiveModel};
use secrecy::ExposeSecret;
use tokio::{runtime::Handle};
use crate::database::secret::Secret;
use crate::database::variables::{GeneralProperties, UserData};

use super::*;

// Get thread specific data
pub fn get_data() -> (Arc<UserData>, Arc<GeneralProperties>) {
    let user = USER_PROPERTIES.with(|data| data.borrow().clone().expect("Failed to get UserData"));
    let properties =
        GENERAL_PROPERTIES.with(|data| data.borrow().clone().expect("Failed to get Properties"));
    (user, properties)
}

// Sets thread specific data, also returns new values
pub async fn set_data(instance: &UserInstanceData) -> (Arc<UserData>, Arc<GeneralProperties>) {
    let user_data = Arc::new(instance.user_data.read().await.clone());
    let settings_data = Arc::new(instance.general_settings.read().await.clone());
    USER_PROPERTIES.with(|data| *data.borrow_mut() = Some(user_data.clone()));
    GENERAL_PROPERTIES.with(|data| *data.borrow_mut() = Some(settings_data.clone()));
    (user_data, settings_data)
}

pub fn get_set_name_local(user: &UserData, set_new_name: Option<String>) -> String {
    // To get the name, first try the new name function body variable.
    // Then try the global variable
    // Then try the Local database variable (which is not set the first time the instance is ever run)
    // So if this is called before the first time the instance is run, it wil return "Onbekend"
    let name = set_new_name
        .as_deref()
        .unwrap_or(
            NAME.get().borrow().as_deref().unwrap_or(
                user.name
                    .as_ref()
                    .and_then(|secret| Some(secret.0.expose_secret()))
                    .unwrap_or(&user.user_name),
            ),
        )
        .to_owned();

    // Open a database connection and write the new name to the database, if a new name request is done
    if let Some(new_name) = set_new_name
        && Some(new_name.as_str()) != NAME.get().borrow().as_deref()
    {
        tokio::task::block_in_place(move || {
            Handle::current().block_on(update_name(new_name, user.id))
        })
        .warn("Setting name");
    }
    NAME.get().replace(Some(name.clone()));
    name
}

pub async fn update_name(new_name: String, data_id: i32) -> Result<()> {
    info!("Changing user name to {new_name}");
    let db = get_database_connection().await;
    let data = user_data::Entity::find_by_id(data_id).one(&db).await?;
    if let Some(model) = data {
        let mut active_model = model.into_active_model();
        active_model.name = Set(Some(Secret::encrypt_value(&new_name)?));
        user_data::Entity::update(active_model)
            .validate()?
            .exec(&db)
            .await?;
        Ok(())
    } else {
        Err(anyhow!("UserData not found"))
    }
}

pub fn get_set_name(set_new_name: Option<String>) -> String {
    let (user, _properties) = get_data();
    get_set_name_local(user.as_ref(), set_new_name)
}