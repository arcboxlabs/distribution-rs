use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(GcJobs::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(GcJobs::Id).text().not_null().primary_key())
                    .col(
                        ColumnDef::new(GcJobs::Status)
                            .text()
                            .not_null()
                            .default("pending"),
                    )
                    .col(ColumnDef::new(GcJobs::StartedAt).text())
                    .col(ColumnDef::new(GcJobs::CompletedAt).text())
                    .col(ColumnDef::new(GcJobs::Stats).text())
                    .col(ColumnDef::new(GcJobs::CreatedAt).text().not_null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(GcJobs::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum GcJobs {
    Table,
    Id,
    Status,
    StartedAt,
    CompletedAt,
    Stats,
    CreatedAt,
}
