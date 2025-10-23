use std::sync::Arc;

use base64::Engine;
use base64::prelude::BASE64_STANDARD_NO_PAD;
use dotenvy::var;
use entity::{
    donation_text, email_properties, general_properties_db, kuma_properties, user_data,
    user_properties,
};
use sea_orm::RelationTrait;
use sea_orm::{ColumnTrait, QuerySelect};
use sea_orm::{DatabaseConnection, DerivePartialModel, EntityTrait, QueryFilter};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::GenResult;

pub type ThreadShare<T> = Arc<RwLock<T>>;

#[derive(Debug, Clone)]
pub struct UserInstanceData {
    pub user_data: ThreadShare<UserData>,
    pub general_settings: ThreadShare<GeneralProperties>,
}

impl UserInstanceData {
    pub async fn load_user(
        db: &DatabaseConnection,
        username: &str,
        default_properties: Arc<RwLock<GeneralProperties>>,
    ) -> GenResult<Option<Self>> {
        let userdata = UserData::get_from_username(db, username).await?;
        if let Some(user_data) = userdata {
            let custom_properties_id = user_data.custom_general_properties.clone();
            let general_settings = if let Some(custom_id) = custom_properties_id
                && let Ok(Some(custom_properties)) = GeneralProperties::get(db, custom_id).await
            {
                Arc::new(RwLock::new(custom_properties))
            } else {
                default_properties
            };
            Ok(Some(Self {
                user_data: Arc::new(RwLock::new(user_data)),
                general_settings,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn update_user(&self, db: &DatabaseConnection) -> GenResult<()> {
        let username = self.user_data.read().await.user_name.clone();
        let userdata = UserData::get_from_username(db, &username).await?;
        if let Some(user_data) = userdata {
            *self.user_data.write().await = user_data.clone();
            let custom_properties_id = user_data.custom_general_properties.clone();
            if let Some(custom_id) = custom_properties_id
                && let Ok(Some(custom_properties)) = GeneralProperties::get(db, custom_id).await
            {
                *self.general_settings.write().await = custom_properties;
            }
        }
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(DerivePartialModel, Debug, Clone)]
#[sea_orm(entity = "general_properties_db::Entity")]
pub struct GeneralProperties {
    pub general_properties_id: i32,
    pub calendar_target: String,
    pub file_target: String,
    pub ical_domain: String,
    pub webcal_domain: String,
    pub pdf_shift_domain: String,
    pub signin_fail_execution_reduce: i32,
    pub signin_fail_mail_reduce: i32,
    pub expected_execution_time_seconds: i32,
    pub execution_retry_count: i32,
    pub support_mail: String,
    pub password_reset_link: String,
    #[sea_orm(nested)]
    pub kuma_properties: KumaProperties,
    #[sea_orm(nested, alias = "general_email")]
    pub general_email_properties: email_properties::Model,
    #[sea_orm(nested)]
    pub donation_text: donation_text::Model,
}

impl GeneralProperties {
    pub async fn get(db: &DatabaseConnection, id: i32) -> GenResult<Option<GeneralProperties>> {
        Ok(general_properties_db::Entity::find_by_id(id)
            .left_join(kuma_properties::Entity)
            .left_join(email_properties::Entity)
            .left_join(donation_text::Entity)
            .join_as(
                sea_orm::JoinType::LeftJoin,
                kuma_properties::Relation::EmailProperties.def(),
                "kuma_email",
            )
            .join_as(
                sea_orm::JoinType::LeftJoin,
                general_properties_db::Relation::EmailProperties.def(),
                "general_email",
            )
            .into_partial_model()
            .one(db)
            .await?)
    }

    pub async fn load_default_preferences(db: &DatabaseConnection) -> GenResult<GeneralProperties> {
        let properties_id = var("DEFAULT_PROPERTIES_ID")
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(1);
        Ok(GeneralProperties::get(db, properties_id)
            .await?
            .expect("No default properties"))
    }
}

#[allow(dead_code)]
#[derive(DerivePartialModel, Debug, Clone)]
#[sea_orm(entity = "kuma_properties::Entity")]
pub struct KumaProperties {
    pub domain: String,
    #[sea_orm(from_col = "kuma_username")]
    pub username: String,
    #[sea_orm(from_col = "kuma_password")]
    pub password: String,
    pub hearbeat_retry: i32,
    pub offline_mail_resend_hours: i32,
    #[sea_orm(nested, alias = "kuma_email")]
    pub kuma_email_properties: email_properties::Model,
    pub mail_port: i32,
    pub use_ssl: bool,
}

#[allow(dead_code)]
#[derive(DerivePartialModel, Debug, Clone, Serialize)]
#[sea_orm(entity = "user_data::Entity")]
pub struct UserData {
    pub user_name: String,
    pub personeelsnummer: String,
    pub password: String,
    pub email: String,
    pub file_name: String,
    #[sea_orm(nested)]
    pub user_properties: user_properties::Model,
    custom_general_properties: Option<i32>,
}

impl UserData {
    pub async fn get_from_username(
        db: &DatabaseConnection,
        username: &str,
    ) -> GenResult<Option<Self>> {
        if let Some(id) = user_data::Entity::find()
            .filter(user_data::Column::UserName.contains(username))
            .column(user_data::Column::UserDataId)
            .into_tuple::<i32>()
            .one(db)
            .await?
        {
            UserData::get_id(db, id).await
        } else {
            Ok(None)
        }
    }
    pub async fn get_id(db: &DatabaseConnection, id: i32) -> GenResult<Option<Self>> {
        let mut userdata = user_data::Entity::find_by_id(id)
            .left_join(user_properties::Entity)
            .left_join(general_properties_db::Entity)
            .join_as(
                sea_orm::JoinType::LeftJoin,
                general_properties_db::Relation::EmailProperties.def(),
                "general_email",
            )
            .into_partial_model::<UserData>()
            .one(db)
            .await?;
        if let Some(data) = userdata.as_mut() {
            data.decrypt_password()?;
        }
        Ok(userdata)
    }

    pub async fn get_all_usernames(db: &DatabaseConnection) -> GenResult<Vec<String>> {
        let data: Vec<String> = user_data::Entity::find()
            .select_only()
            .column(user_data::Column::UserName)
            .into_tuple()
            .all(db)
            .await?;
        Ok(data)
    }

    fn decrypt_password(&mut self) -> GenResult<()> {
        let secret_string = var("PASSWORD_SECRET")?;
        let secret = secret_string.as_bytes();
        self.password = String::from_utf8(
            simplestcrypt::deserialize_and_decrypt(
                secret,
                &BASE64_STANDARD_NO_PAD.decode(&self.password)?,
            )
            .unwrap(),
        )?;
        Ok(())
    }
}
