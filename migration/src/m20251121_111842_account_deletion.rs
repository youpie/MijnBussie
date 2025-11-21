use sea_orm_migration::prelude::*;

use crate::{
    m20251006_143409_general_settings::GeneralPropertiesDB,
    m20251008_194017_user_settings::UserProperties, m20251008_194417_user_data::UserData,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Replace the sample below with your own migration scripts

        manager
            .alter_table(
                Table::alter()
                    .table(UserData::Table)
                    .add_column(
                        ColumnDef::new_with_type(
                            UserData::LastSuccesfullSignInDate,
                            ColumnType::Timestamp,
                        )
                        .null(),
                    )
                    .add_column(
                        ColumnDef::new_with_type(
                            UserData::LastExecutionDate,
                            ColumnType::Timestamp,
                        )
                        .null(),
                    )
                    .add_column(
                        ColumnDef::new_with_type(UserData::CreationDate, ColumnType::Timestamp)
                            .not_null()
                            .default(chrono::offset::Utc::now().naive_utc()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(UserProperties::Table)
                    .add_column(
                        ColumnDef::new_with_type(
                            UserProperties::AutoDeleteAccount,
                            ColumnType::Boolean,
                        )
                        .default(true),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(GeneralPropertiesDB::Table)
                    .add_column(
                        ColumnDef::new_with_type(GeneralPropertiesDB::SignUpUrl, ColumnType::Text)
                            .not_null()
                            .default("https://link.bussie.app/Aanmelden"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Replace the sample below with your own migration scripts
        manager
            .alter_table(
                Table::alter()
                    .table(UserProperties::Table)
                    .drop_column(UserProperties::AutoDeleteAccount)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(GeneralPropertiesDB::Table)
                    .drop_column(GeneralPropertiesDB::SignUpUrl)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(UserData::Table)
                    .drop_column(UserData::LastSuccesfullSignInDate)
                    .drop_column(UserData::LastExecutionDate)
                    .drop_column(UserData::CreationDate)
                    .to_owned(),
            )
            .await
    }
}
