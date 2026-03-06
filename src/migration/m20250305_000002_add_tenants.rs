use sea_orm_migration::prelude::*;

#[derive(DeriveIden)]
enum Tenants {
    Table,
    Id,
    Name,
    Status,
    CreatedAt,
}

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Tenants::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Tenants::Id).text().not_null().primary_key())
                    .col(ColumnDef::new(Tenants::Name).text().not_null())
                    .col(
                        ColumnDef::new(Tenants::Status)
                            .text()
                            .not_null()
                            .default("active"),
                    )
                    .col(ColumnDef::new(Tenants::CreatedAt).text().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO tenants (id, name, status, created_at) VALUES ('_default', 'Default', 'active', '2025-03-05T00:00:00Z')",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Tenants::Table).to_owned())
            .await
    }
}
