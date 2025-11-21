use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

use super::m20251006_130009_email::EmailProperties;
use super::m20251006_140009_donation::DonationText;
use super::m20251006_141509_kuma::KumaProperties;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(GeneralPropertiesDB::Table)
                    .if_not_exists()
                    .col(pk_auto(GeneralPropertiesDB::GeneralPropertiesId))
                    .col(string(GeneralPropertiesDB::CalendarTarget))
                    .col(string(GeneralPropertiesDB::FileTarget))
                    .col(string(GeneralPropertiesDB::IcalDomain))
                    .col(string(GeneralPropertiesDB::WebcalDomain))
                    .col(string(GeneralPropertiesDB::PDFShiftDomain))
                    .col(integer(GeneralPropertiesDB::SigninFailExecutionReduce))
                    .col(integer(GeneralPropertiesDB::SigninFailMailReduce))
                    .col(integer(GeneralPropertiesDB::ExpectedExecutionTimeSeconds))
                    .col(integer(GeneralPropertiesDB::ExecutionRetryCount))
                    .col(string(GeneralPropertiesDB::SupportMail))
                    .col(string(GeneralPropertiesDB::PasswordResetLink))
                    .col(integer(GeneralPropertiesDB::KumaProperties).default(1))
                    .col(integer(GeneralPropertiesDB::GeneralEmailProperties).default(1))
                    .col(integer(GeneralPropertiesDB::DonationText).default(1))
                    .foreign_key(
                        ForeignKey::create()
                            .name("kuma_properties_fk")
                            .from(
                                GeneralPropertiesDB::Table,
                                GeneralPropertiesDB::KumaProperties,
                            )
                            .to(KumaProperties::Table, KumaProperties::KumaId)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("email_properties_general_fk")
                            .from(
                                GeneralPropertiesDB::Table,
                                GeneralPropertiesDB::GeneralEmailProperties,
                            )
                            .to(EmailProperties::Table, EmailProperties::EmailId)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("donation_text_fk")
                            .from(
                                GeneralPropertiesDB::Table,
                                GeneralPropertiesDB::DonationText,
                            )
                            .to(DonationText::Table, DonationText::DonationId)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(GeneralPropertiesDB::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum GeneralPropertiesDB {
    Table,
    GeneralPropertiesId,
    CalendarTarget,
    FileTarget,
    IcalDomain,
    WebcalDomain,
    PDFShiftDomain,
    SigninFailExecutionReduce,
    SigninFailMailReduce,
    ExpectedExecutionTimeSeconds,
    ExecutionRetryCount,
    SupportMail,
    PasswordResetLink,
    GeneralEmailProperties,
    KumaProperties,
    DonationText,

    SignUpUrl,
}
