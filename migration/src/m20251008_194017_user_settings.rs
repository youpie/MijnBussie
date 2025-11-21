use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserProperties::Table)
                    .if_not_exists()
                    .col(pk_auto(UserProperties::UserPropertiesId))
                    .col(integer(UserProperties::ExecutionIntervalMinutes).default(7200))
                    .col(integer(UserProperties::ExecutionMinute).default(0))
                    .col(boolean(UserProperties::SendMailNewShift).default(false))
                    .col(boolean(UserProperties::SendMailUpdatedShift).default(false))
                    .col(boolean(UserProperties::SendMailRemovedShift).default(false))
                    .col(boolean(UserProperties::SendFailedSigninMail).default(false))
                    .col(boolean(UserProperties::SendWelcomeMail).default(false))
                    .col(boolean(UserProperties::SendErrorMail).default(false))
                    .col(boolean(UserProperties::SplitNightShift).default(false))
                    .col(boolean(UserProperties::StopMidnightShift).default(false))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserProperties::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum UserProperties {
    Table,
    UserPropertiesId,
    ExecutionIntervalMinutes,
    ExecutionMinute,
    SendMailNewShift,
    SendMailUpdatedShift,
    SendMailRemovedShift,
    SendFailedSigninMail,
    SendWelcomeMail,
    SendErrorMail,
    SplitNightShift,
    StopMidnightShift,

    AutoDeleteAccount,
}
