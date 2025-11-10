use sea_orm_migration::{prelude::*, schema::*};

use crate::m20251008_194417_user_data::UserData;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Replace the sample below with your own migration scripts

        manager
            .create_table(
                Table::create()
                    .table(UserAccount::Table)
                    .if_not_exists()
                    .col(pk_auto(UserAccount::AccountId))
                    .col(string(UserAccount::Username).unique_key())
                    .col(string(UserAccount::PasswordHash))
                    .col(string(UserAccount::Role))
                    .col(string(UserAccount::BackendUser).null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("user_data_fk")
                            .from(UserAccount::Table, UserAccount::BackendUser)
                            .to(UserData::Table, UserData::UserName)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Replace the sample below with your own migration scripts

        manager
            .drop_table(Table::drop().table(UserAccount::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum UserAccount {
    Table,
    AccountId,
    Username,
    PasswordHash,
    Role,
    BackendUser,
}
