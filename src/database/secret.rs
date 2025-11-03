use base64::{Engine, prelude::BASE64_STANDARD_NO_PAD};
use dotenvy::var;
use sea_orm::{QueryResult, TryGetError, TryGetable, Value, sea_query};
use secrecy::{ExposeSecret, SecretString};
use serde::{Serialize, Serializer};

use crate::{GenResult, errors::OptionResult};

/// A `SecretString` wrapper that automatically decodes using the `$PASSWORD_SECRET`
#[derive(Clone, Debug)]
pub struct Secret(pub SecretString);

// 1) From<Secret> for sea_orm::Value
impl From<Secret> for Value {
    fn from(source: Secret) -> Self {
        // careful: this exposes the inner secret as plain text for storage
        Secret::encrypt_value(source.0.expose_secret())
            .unwrap_or("Error".to_owned())
            .into()
    }
}

// 2) sea_orm::TryGetable for Secret (how to read from QueryResult)
impl TryGetable for Secret {
    fn try_get_by<I: sea_orm::ColIdx>(
        res: &QueryResult,
        idx: I,
    ) -> Result<Self, sea_orm::TryGetError> {
        // delegate to String's TryGetable then wrap
        let string = <String as TryGetable>::try_get_by(res, idx)?;
        Ok(Secret(SecretString::new(
            Self::decrypt_value(string)
                .map_err(|err| TryGetError::Null(err.to_string()))?
                .into(),
        )))
    }
}

// 3) sea_query::ValueType for Secret (converts sea_orm::Value -> Secret)
impl sea_query::ValueType for Secret {
    fn try_from(v: Value) -> Result<Self, sea_query::ValueTypeErr> {
        // delegate to String's conversion then wrap
        let string = <String as sea_query::ValueType>::try_from(v)?;
        Ok(Secret(SecretString::new(
            Self::decrypt_value(string)
                .map_err(|_| sea_query::ValueTypeErr)?
                .into(),
        )))
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

impl Secret {
    fn decrypt_value(value: String) -> GenResult<String> {
        let secret_string = var("PASSWORD_SECRET")?;
        let secret = secret_string.as_bytes();
        let value = String::from_utf8(
            simplestcrypt::deserialize_and_decrypt(secret, &BASE64_STANDARD_NO_PAD.decode(value)?)
                .ok()
                .result_reason("Could not deserialize password")?,
        )?;
        Ok(value)
    }

    pub fn encrypt_value(value: &str) -> GenResult<String> {
        let secret_string = var("PASSWORD_SECRET")?;
        let secret = secret_string.as_bytes();
        let value = BASE64_STANDARD_NO_PAD.encode(
            simplestcrypt::encrypt_and_serialize(secret, value.as_bytes())
                .ok()
                .result_reason("Failed to encode password")?,
        );
        Ok(value)
    }
}
