use sea_orm_migration::prelude::*;

use crate::m20251008_194417_user_data::UserData;

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
                            UserData::LastSystemExecutionDate,
                            ColumnType::Timestamp,
                        )
                        .null(),
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
                    .table(UserData::Table)
                    .drop_column(UserData::LastSystemExecutionDate)
                    .to_owned(),
            )
            .await
    }
}
