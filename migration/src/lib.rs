pub use sea_orm_migration::prelude::*;

mod m20251006_130009_email;
mod m20251006_140009_donation;
mod m20251006_141509_kuma;
mod m20251006_143409_general_settings;
mod m20251008_194017_user_settings;
mod m20251008_194417_user_data;
mod m20251110_155639_user_account;
mod m20251115_110830_name;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20251006_130009_email::Migration),
            Box::new(m20251006_140009_donation::Migration),
            Box::new(m20251006_141509_kuma::Migration),
            Box::new(m20251006_143409_general_settings::Migration),
            Box::new(m20251008_194017_user_settings::Migration),
            Box::new(m20251008_194417_user_data::Migration),
            Box::new(m20251110_155639_user_account::Migration),
            Box::new(m20251115_110830_name::Migration),
        ]
    }
}
