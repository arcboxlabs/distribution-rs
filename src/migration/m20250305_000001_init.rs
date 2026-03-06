use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[allow(clippy::too_many_lines)]
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. repositories
        manager
            .create_table(
                Table::create()
                    .table(Repositories::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Repositories::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Repositories::TenantId)
                            .text()
                            .not_null()
                            .default("_default"),
                    )
                    .col(ColumnDef::new(Repositories::Name).text().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_repositories_tenant_name")
                    .table(Repositories::Table)
                    .col(Repositories::TenantId)
                    .col(Repositories::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // 2. repo_blob_links
        manager
            .create_table(
                Table::create()
                    .table(RepoBlobLinks::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(RepoBlobLinks::RepoId).integer().not_null())
                    .col(ColumnDef::new(RepoBlobLinks::Digest).text().not_null())
                    .col(ColumnDef::new(RepoBlobLinks::Size).big_integer().not_null())
                    .primary_key(
                        Index::create()
                            .col(RepoBlobLinks::RepoId)
                            .col(RepoBlobLinks::Digest),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(RepoBlobLinks::Table, RepoBlobLinks::RepoId)
                            .to(Repositories::Table, Repositories::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // 3. manifests
        manager
            .create_table(
                Table::create()
                    .table(Manifests::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Manifests::RepoId).integer().not_null())
                    .col(ColumnDef::new(Manifests::Digest).text().not_null())
                    .col(ColumnDef::new(Manifests::MediaType).text().not_null())
                    .col(ColumnDef::new(Manifests::Size).big_integer().not_null())
                    .primary_key(
                        Index::create()
                            .col(Manifests::RepoId)
                            .col(Manifests::Digest),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Manifests::Table, Manifests::RepoId)
                            .to(Repositories::Table, Repositories::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // 4. tags
        manager
            .create_table(
                Table::create()
                    .table(Tags::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Tags::RepoId).integer().not_null())
                    .col(ColumnDef::new(Tags::Name).text().not_null())
                    .col(ColumnDef::new(Tags::Digest).text().not_null())
                    .col(ColumnDef::new(Tags::UpdatedAt).text().not_null())
                    .col(ColumnDef::new(Tags::LastPulledAt).text())
                    .col(
                        ColumnDef::new(Tags::PullCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .primary_key(Index::create().col(Tags::RepoId).col(Tags::Name))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Tags::Table, Tags::RepoId)
                            .to(Repositories::Table, Repositories::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // 5. upload_sessions
        manager
            .create_table(
                Table::create()
                    .table(UploadSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UploadSessions::Id)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UploadSessions::RepoId).integer().not_null())
                    .col(
                        ColumnDef::new(UploadSessions::Offset)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(UploadSessions::CreatedAt).text().not_null())
                    .col(ColumnDef::new(UploadSessions::UpdatedAt).text().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(UploadSessions::Table, UploadSessions::RepoId)
                            .to(Repositories::Table, Repositories::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // 6. referrers
        manager
            .create_table(
                Table::create()
                    .table(Referrers::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Referrers::RepoId).integer().not_null())
                    .col(ColumnDef::new(Referrers::SubjectDigest).text().not_null())
                    .col(ColumnDef::new(Referrers::ReferrerDigest).text().not_null())
                    .col(ColumnDef::new(Referrers::ArtifactType).text())
                    .primary_key(
                        Index::create()
                            .col(Referrers::RepoId)
                            .col(Referrers::SubjectDigest)
                            .col(Referrers::ReferrerDigest),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Referrers::Table, Referrers::RepoId)
                            .to(Repositories::Table, Repositories::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Referrers::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(UploadSessions::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Tags::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Manifests::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(RepoBlobLinks::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Repositories::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Repositories {
    Table,
    Id,
    TenantId,
    Name,
}

#[derive(DeriveIden)]
enum RepoBlobLinks {
    Table,
    RepoId,
    Digest,
    Size,
}

#[derive(DeriveIden)]
enum Manifests {
    Table,
    RepoId,
    Digest,
    MediaType,
    Size,
}

#[derive(DeriveIden)]
enum Tags {
    Table,
    RepoId,
    Name,
    Digest,
    UpdatedAt,
    LastPulledAt,
    PullCount,
}

#[derive(DeriveIden)]
enum UploadSessions {
    Table,
    Id,
    RepoId,
    Offset,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Referrers {
    Table,
    RepoId,
    SubjectDigest,
    ReferrerDigest,
    ArtifactType,
}
