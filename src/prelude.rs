pub use anyhow::Result;
pub use anyhow::anyhow;
pub use tracing::*;
pub use crate::errors::*;
pub use crate::instance::data::get_data;
pub use crate::create_path;

pub type GenResult<T> = Result<T, GenError>;
pub type GenError = anyhow::Error;