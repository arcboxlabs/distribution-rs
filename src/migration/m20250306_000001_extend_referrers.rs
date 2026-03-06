use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Referrers::Table)
                    .add_column(
                        ColumnDef::new(Referrers::MediaType)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Referrers::Table)
                    .add_column(
                        ColumnDef::new(Referrers::Size)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Referrers::Table)
                    .add_column(ColumnDef::new(Referrers::Annotations).text())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite doesn't support DROP COLUMN before 3.35, so we just leave them.
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Referrers {
    Table,
    MediaType,
    Size,
    Annotations,
}
