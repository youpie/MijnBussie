use dotenvy::var;
use sea_orm::{Database, DatabaseConnection};

pub mod secret;
pub mod variables;

pub async fn get_database_connection() -> DatabaseConnection {
    Database::connect(&var("DATABASE_URL").expect("Failed to get database URL"))
        .await
        .expect("Could not connect to database")
}