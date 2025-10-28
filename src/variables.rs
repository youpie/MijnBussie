use std::sync::Arc;

use base64::Engine;
use base64::prelude::BASE64_STANDARD_NO_PAD;
use dotenvy::var;
use entity::{
    donation_text, email_properties, general_properties_db, kuma_properties, user_data,
    user_properties,
};
use sea_orm::{ColumnTrait, QuerySelect};
use sea_orm::{DatabaseConnection, DerivePartialModel, EntityTrait, QueryFilter};
use sea_orm::{QueryResult, RelationTrait, TryGetable, Value, sea_query};
use secrecy::{ExposeSecret, SecretString};
use serde::{Serialize, Serializer};
use tokio::sync::RwLock;

use crate::GenResult;

pub type ThreadShare<T> = Arc<RwLock<T>>;

#[derive(Debug, Clone)]
pub struct UserInstanceData {
    pub user_data: ThreadShare<UserData>,
    pub general_settings: ThreadShare<GeneralProperties>,
}

impl UserInstanceData {
    pub async fn get_data_local(&self) -> (UserData, GeneralProperties) {
        (
            self.user_data.read().await.clone(),
            self.general_settings.read().await.clone(),
        )
    }

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

#[derive(Clone, Debug)]
pub struct Secret(pub SecretString);

// 1) From<Secret> for sea_orm::Value
impl From<Secret> for Value {
    fn from(source: Secret) -> Self {
        // careful: this exposes the inner secret as plain text for storage
        source.0.expose_secret().to_owned().into()
    }
}

// 2) sea_orm::TryGetable for Secret (how to read from QueryResult)
impl TryGetable for Secret {
    fn try_get_by<I: sea_orm::ColIdx>(
        res: &QueryResult,
        idx: I,
    ) -> Result<Self, sea_orm::TryGetError> {
        // delegate to String's TryGetable then wrap
        <String as TryGetable>::try_get_by(res, idx).map(|s| Secret(SecretString::new(s.into())))
    }
}

// 3) sea_query::ValueType for Secret (converts sea_orm::Value -> Secret)
impl sea_query::ValueType for Secret {
    fn try_from(v: Value) -> Result<Self, sea_query::ValueTypeErr> {
        // delegate to String's conversion then wrap
        <String as sea_query::ValueType>::try_from(v).map(|s| Secret(SecretString::new(s.into())))
    }

    fn type_name() -> String {
        stringify!(Secret).to_owned()
    }

    fn array_type() -> sea_query::ArrayType {
        sea_query::ArrayType::String
    }

    fn column_type() -> sea_query::ColumnType {
        // unbounded string column type; adjust if you want a length
        sea_query::ColumnType::String(sea_query::StringLen::None)
    }
}

// 4) sea_query::Nullable for Secret
impl sea_query::Nullable for Secret {
    fn null() -> Value {
        // delegate to String's `null()`
        <String as sea_query::Nullable>::null()
    }
}

impl Serialize for Secret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // redact the value but keep length info if you like
        let len = self.0.expose_secret().len();
        serializer.serialize_str(&format!("[REDACTED, {} bytes]", len))
    }
}

#[allow(dead_code)]
#[derive(DerivePartialModel, Debug, Clone, Serialize)]
#[sea_orm(entity = "user_data::Entity")]
pub struct UserData {
    pub user_name: String,
    pub personeelsnummer: String,
    pub password: Secret,
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
        self.password = Secret(SecretString::new(
            String::from_utf8(
                simplestcrypt::deserialize_and_decrypt(
                    secret,
                    &BASE64_STANDARD_NO_PAD.decode(self.password.0.expose_secret())?,
                )
                .unwrap(),
            )?
            .into(),
        ));
        Ok(())
    }
}
