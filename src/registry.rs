use crate::entity::{gc_job, manifest, referrer, repo_blob_link, repository, tag, upload_session};
use crate::error::{ErrorCode, OciError, RegistryError};
use crate::storage::{BoxAsyncRead, Storage};
use crate::types::{Digest, Reference, RepoName, TenantId, UploadId};
use oci_spec::image::{ImageIndex, ImageManifest};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set, TransactionTrait, TryInsertResult,
};
use sha2::{Digest as Sha256Digest, Sha256};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

pub struct Registry {
    db: DatabaseConnection,
    storage: Arc<dyn Storage>,
}

impl Registry {
    pub fn new(db: DatabaseConnection, storage: Arc<dyn Storage>) -> Self {
        Self { db, storage }
    }

    /// Clean up orphan upload and tmp files left from a previous run.
    /// Call once after construction, before serving requests.
    pub async fn startup_cleanup(&self) -> Result<(), RegistryError> {
        // 1. Remove all tmp files (always orphans from incomplete writes).
        let tmp_removed = self
            .storage
            .cleanup_tmp()
            .await
            .map_err(RegistryError::Storage)?;
        if tmp_removed > 0 {
            tracing::info!(tmp_removed, "cleaned up orphan tmp files");
        }

        // 2. Remove upload files that have no matching DB session.
        let storage_ids = self
            .storage
            .list_upload_ids()
            .await
            .map_err(RegistryError::Storage)?;
        let mut orphan_count: u64 = 0;
        for id_str in &storage_ids {
            let exists = upload_session::Entity::find_by_id(id_str)
                .one(&self.db)
                .await?
                .is_some();
            if !exists {
                if let Ok(uid) = id_str.parse::<UploadId>() {
                    let _ = self.storage.abort_upload(&uid).await;
                    orphan_count += 1;
                }
            }
        }
        if orphan_count > 0 {
            tracing::info!(orphan_count, "cleaned up orphan upload files");
        }

        Ok(())
    }

    async fn compute_digest(mut reader: BoxAsyncRead) -> Result<Digest, RegistryError> {
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader
                .read(&mut buf)
                .await
                .map_err(|e| RegistryError::Storage(e.into()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let hash = hasher.finalize();
        let hex = hex::encode(hash);
        let digest_str = format!("sha256:{hex}");
        digest_str
            .parse::<Digest>()
            .map_err(|e| RegistryError::Storage(anyhow::anyhow!("{e}")))
    }

    pub async fn health_check(&self) -> Result<(), RegistryError> {
        use sea_orm::ConnectionTrait;
        self.db.execute_unprepared("SELECT 1").await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Repository management
    // -----------------------------------------------------------------------

    pub async fn ensure_repository(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
    ) -> Result<i32, RegistryError> {
        // Try to find existing first (fast path).
        if let Some(row) = repository::Entity::find()
            .filter(repository::Column::TenantId.eq(tenant_id.as_str()))
            .filter(repository::Column::Name.eq(name.as_str()))
            .one(&self.db)
            .await?
        {
            return Ok(row.id);
        }

        // Insert with on_conflict ignore so concurrent callers don't race.
        let model = repository::ActiveModel {
            id: ActiveValue::default(),
            tenant_id: Set(tenant_id.as_str().to_owned()),
            name: Set(name.as_str().to_owned()),
        };
        let insert = repository::Entity::insert(model)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    repository::Column::TenantId,
                    repository::Column::Name,
                ])
                .do_nothing()
                .to_owned(),
            )
            .do_nothing()
            .exec(&self.db)
            .await?;

        match insert {
            TryInsertResult::Inserted(result) => return Ok(result.last_insert_id),
            TryInsertResult::Conflicted | TryInsertResult::Empty => {}
        }

        // Conflict path – row already exists, select it.
        let row = repository::Entity::find()
            .filter(repository::Column::TenantId.eq(tenant_id.as_str()))
            .filter(repository::Column::Name.eq(name.as_str()))
            .one(&self.db)
            .await?
            .ok_or_else(|| OciError::name_unknown(name.as_str()))?;

        Ok(row.id)
    }

    /// Resolve `repo_id`, returning `NAME_UNKNOWN` when the repository doesn't exist.
    async fn require_repo(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
    ) -> Result<i32, RegistryError> {
        repository::Entity::find()
            .filter(repository::Column::TenantId.eq(tenant_id.as_str()))
            .filter(repository::Column::Name.eq(name.as_str()))
            .one(&self.db)
            .await?
            .map(|r| r.id)
            .ok_or_else(|| OciError::name_unknown(name.as_str()).into())
    }

    // -----------------------------------------------------------------------
    // Blob operations
    // -----------------------------------------------------------------------

    pub async fn blob_exists(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        digest: &Digest,
    ) -> Result<bool, RegistryError> {
        let Ok(repo_id) = self.require_repo(tenant_id, name).await else {
            return Ok(false);
        };

        let link = repo_blob_link::Entity::find()
            .filter(repo_blob_link::Column::RepoId.eq(repo_id))
            .filter(repo_blob_link::Column::Digest.eq(digest.to_string()))
            .one(&self.db)
            .await?;

        Ok(link.is_some())
    }

    pub async fn get_blob(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        digest: &Digest,
    ) -> Result<(BoxAsyncRead, u64), RegistryError> {
        let repo_id = self.require_repo(tenant_id, name).await?;

        let link = repo_blob_link::Entity::find()
            .filter(repo_blob_link::Column::RepoId.eq(repo_id))
            .filter(repo_blob_link::Column::Digest.eq(digest.to_string()))
            .one(&self.db)
            .await?
            .ok_or_else(OciError::blob_unknown)?;

        let reader = self.storage.get_blob(digest).await?;
        #[allow(clippy::cast_sign_loss)]
        let size = link.size as u64;
        Ok((reader, size))
    }

    pub async fn get_blob_range(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        digest: &Digest,
        offset: u64,
        length: u64,
    ) -> Result<BoxAsyncRead, RegistryError> {
        let repo_id = self.require_repo(tenant_id, name).await?;

        let _link = repo_blob_link::Entity::find()
            .filter(repo_blob_link::Column::RepoId.eq(repo_id))
            .filter(repo_blob_link::Column::Digest.eq(digest.to_string()))
            .one(&self.db)
            .await?
            .ok_or_else(OciError::blob_unknown)?;

        let reader = self.storage.get_blob_range(digest, offset, length).await?;
        Ok(reader)
    }

    pub async fn blob_head(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        digest: &Digest,
    ) -> Result<u64, RegistryError> {
        let repo_id = self.require_repo(tenant_id, name).await?;
        let link = repo_blob_link::Entity::find()
            .filter(repo_blob_link::Column::RepoId.eq(repo_id))
            .filter(repo_blob_link::Column::Digest.eq(digest.to_string()))
            .one(&self.db)
            .await?
            .ok_or_else(OciError::blob_unknown)?;
        #[allow(clippy::cast_sign_loss)]
        Ok(link.size as u64)
    }

    pub async fn mount_blob(
        &self,
        tenant_id: &TenantId,
        to_name: &RepoName,
        from_name: &RepoName,
        digest: &Digest,
    ) -> Result<bool, RegistryError> {
        // Look up source repo; if it doesn't exist the mount simply fails.
        let Ok(source_repo_id) = self.require_repo(tenant_id, from_name).await else {
            return Ok(false);
        };

        // Check that the blob is linked in the source repo.
        let Some(source_link) = repo_blob_link::Entity::find()
            .filter(repo_blob_link::Column::RepoId.eq(source_repo_id))
            .filter(repo_blob_link::Column::Digest.eq(digest.to_string()))
            .one(&self.db)
            .await?
        else {
            return Ok(false);
        };

        // Ensure destination repo exists.
        let dest_repo_id = self.ensure_repository(tenant_id, to_name).await?;

        // Link the blob to the destination repo (idempotent).
        let link = repo_blob_link::ActiveModel {
            repo_id: Set(dest_repo_id),
            digest: Set(digest.to_string()),
            size: Set(source_link.size),
        };
        repo_blob_link::Entity::insert(link)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    repo_blob_link::Column::RepoId,
                    repo_blob_link::Column::Digest,
                ])
                .update_column(repo_blob_link::Column::Size)
                .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(true)
    }

    pub async fn delete_blob(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        digest: &Digest,
    ) -> Result<(), RegistryError> {
        let repo_id = self.require_repo(tenant_id, name).await?;

        let res = repo_blob_link::Entity::delete_many()
            .filter(repo_blob_link::Column::RepoId.eq(repo_id))
            .filter(repo_blob_link::Column::Digest.eq(digest.to_string()))
            .exec(&self.db)
            .await?;

        if res.rows_affected == 0 {
            return Err(OciError::blob_unknown().into());
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Manifest operations
    // -----------------------------------------------------------------------

    /// Resolve a reference to a digest. For `Tag`, looks up the tag table.
    async fn resolve_digest(
        &self,
        repo_id: i32,
        reference: &Reference,
    ) -> Result<Digest, RegistryError> {
        match reference {
            Reference::Digest(d) => Ok(d.clone()),
            Reference::Tag(tag_name) => {
                let row = tag::Entity::find()
                    .filter(tag::Column::RepoId.eq(repo_id))
                    .filter(tag::Column::Name.eq(tag_name.as_str()))
                    .one(&self.db)
                    .await?
                    .ok_or_else(|| OciError::manifest_unknown(tag_name.as_str()))?;
                row.digest
                    .parse::<Digest>()
                    .map_err(|e| OciError::manifest_unknown(e.to_string()).into())
            }
        }
    }

    async fn validate_manifest_references(
        &self,
        repo_id: i32,
        data: &[u8],
        media_type: &str,
    ) -> Result<(), RegistryError> {
        match media_type {
            "application/vnd.oci.image.manifest.v1+json"
            | "application/vnd.docker.distribution.manifest.v2+json" => {
                let manifest: ImageManifest = serde_json::from_slice(data)
                    .map_err(|e| OciError::manifest_invalid(e.to_string()))?;

                let config_digest = manifest.config().digest().to_string();
                let exists = repo_blob_link::Entity::find()
                    .filter(repo_blob_link::Column::RepoId.eq(repo_id))
                    .filter(repo_blob_link::Column::Digest.eq(&config_digest))
                    .one(&self.db)
                    .await?;
                if exists.is_none() {
                    return Err(OciError::manifest_blob_unknown(&config_digest).into());
                }

                for layer in manifest.layers() {
                    let layer_digest = layer.digest().to_string();
                    let exists = repo_blob_link::Entity::find()
                        .filter(repo_blob_link::Column::RepoId.eq(repo_id))
                        .filter(repo_blob_link::Column::Digest.eq(&layer_digest))
                        .one(&self.db)
                        .await?;
                    if exists.is_none() {
                        return Err(OciError::manifest_blob_unknown(&layer_digest).into());
                    }
                }
            }
            "application/vnd.oci.image.index.v1+json"
            | "application/vnd.docker.distribution.manifest.list.v2+json" => {
                let index: ImageIndex = serde_json::from_slice(data)
                    .map_err(|e| OciError::manifest_invalid(e.to_string()))?;

                for sub in index.manifests() {
                    let sub_digest = sub.digest().to_string();
                    let exists = manifest::Entity::find()
                        .filter(manifest::Column::RepoId.eq(repo_id))
                        .filter(manifest::Column::Digest.eq(&sub_digest))
                        .one(&self.db)
                        .await?;
                    if exists.is_none() {
                        return Err(OciError::manifest_blob_unknown(&sub_digest).into());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn get_manifest(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        reference: &Reference,
    ) -> Result<(Vec<u8>, String, Digest), RegistryError> {
        let repo_id = self.require_repo(tenant_id, name).await?;
        let digest = self.resolve_digest(repo_id, reference).await?;

        let row = manifest::Entity::find()
            .filter(manifest::Column::RepoId.eq(repo_id))
            .filter(manifest::Column::Digest.eq(digest.to_string()))
            .one(&self.db)
            .await?
            .ok_or_else(|| OciError::manifest_unknown(reference.to_string()))?;

        let bytes = self.storage.get_manifest_bytes(&digest).await?;

        // Update pull statistics on tags pointing to this digest.
        let now = chrono::Utc::now().to_rfc3339();
        tag::Entity::update_many()
            .col_expr(tag::Column::LastPulledAt, Expr::value(now))
            .col_expr(
                tag::Column::PullCount,
                Expr::col(tag::Column::PullCount).add(1),
            )
            .filter(tag::Column::RepoId.eq(repo_id))
            .filter(tag::Column::Digest.eq(digest.to_string()))
            .exec(&self.db)
            .await?;

        Ok((bytes, row.media_type, digest))
    }

    #[allow(clippy::too_many_lines)]
    pub async fn put_manifest(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        reference: &Reference,
        media_type: &str,
        data: &[u8],
    ) -> Result<(Digest, Option<String>), RegistryError> {
        let hash = Sha256::digest(data);
        let hex = hex::encode(hash);
        let digest_str = format!("sha256:{hex}");
        let digest: Digest = digest_str
            .parse()
            .map_err(|e: crate::types::TypesError| OciError::digest_invalid(e.to_string()))?;

        // If reference is a digest, verify it matches the computed digest
        if let Reference::Digest(ref expected) = *reference {
            if *expected != digest {
                return Err(OciError::digest_invalid(format!(
                    "body digest {digest} does not match reference {expected}"
                ))
                .into());
            }
        }

        // Ensure repo exists and validate references BEFORE writing to storage.
        let repo_id = self.ensure_repository(tenant_id, name).await?;
        self.validate_manifest_references(repo_id, data, media_type)
            .await?;

        // Write manifest bytes to storage.
        self.storage.put_manifest_bytes(&digest, data).await?;

        #[allow(clippy::cast_possible_wrap)]
        let size = data.len() as i64;
        let now = chrono::Utc::now().to_rfc3339();

        // DB transaction: upsert manifest, optionally upsert tag, track referrers.
        let txn = self.db.begin().await?;

        // Upsert manifest.
        let manifest_model = manifest::ActiveModel {
            repo_id: Set(repo_id),
            digest: Set(digest.to_string()),
            media_type: Set(media_type.to_owned()),
            size: Set(size),
        };
        manifest::Entity::insert(manifest_model)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    manifest::Column::RepoId,
                    manifest::Column::Digest,
                ])
                .update_columns([manifest::Column::MediaType, manifest::Column::Size])
                .to_owned(),
            )
            .exec(&txn)
            .await?;

        // If reference is a tag, upsert the tag row.
        if let Reference::Tag(tag_name) = reference {
            let tag_model = tag::ActiveModel {
                repo_id: Set(repo_id),
                name: Set(tag_name.as_str().to_owned()),
                digest: Set(digest.to_string()),
                updated_at: Set(now.clone()),
                last_pulled_at: Set(None),
                pull_count: Set(0),
            };
            tag::Entity::insert(tag_model)
                .on_conflict(
                    sea_orm::sea_query::OnConflict::columns([
                        tag::Column::RepoId,
                        tag::Column::Name,
                    ])
                    .update_columns([tag::Column::Digest, tag::Column::UpdatedAt])
                    .to_owned(),
                )
                .exec(&txn)
                .await?;
        }

        // Track referrers: if the manifest has a `subject` field, record the relationship.
        let manifest_value: serde_json::Value = serde_json::from_slice(data).unwrap_or_default();
        let subject_digest_str = manifest_value
            .get("subject")
            .and_then(|s| s.get("digest"))
            .and_then(|d| d.as_str())
            .map(String::from);

        if let Some(ref subject_digest) = subject_digest_str {
            let artifact_type = manifest_value
                .get("artifactType")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| {
                    manifest_value
                        .get("config")
                        .and_then(|c| c.get("mediaType"))
                        .and_then(|m| m.as_str())
                        .map(String::from)
                });

            let annotations = manifest_value
                .get("annotations")
                .map(|a| serde_json::to_string(a).unwrap_or_default());

            let referrer_model = referrer::ActiveModel {
                repo_id: Set(repo_id),
                subject_digest: Set(subject_digest.clone()),
                referrer_digest: Set(digest.to_string()),
                artifact_type: Set(artifact_type),
                media_type: Set(media_type.to_owned()),
                size: Set(size),
                annotations: Set(annotations),
            };
            referrer::Entity::insert(referrer_model)
                .on_conflict(
                    sea_orm::sea_query::OnConflict::columns([
                        referrer::Column::RepoId,
                        referrer::Column::SubjectDigest,
                        referrer::Column::ReferrerDigest,
                    ])
                    .update_columns([
                        referrer::Column::ArtifactType,
                        referrer::Column::MediaType,
                        referrer::Column::Size,
                        referrer::Column::Annotations,
                    ])
                    .to_owned(),
                )
                .exec(&txn)
                .await?;
        }

        txn.commit().await?;
        Ok((digest, subject_digest_str))
    }

    pub async fn delete_manifest(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        digest: &Digest,
    ) -> Result<(), RegistryError> {
        let repo_id = self.require_repo(tenant_id, name).await?;

        let txn = self.db.begin().await?;

        // Delete tags pointing to this digest.
        tag::Entity::delete_many()
            .filter(tag::Column::RepoId.eq(repo_id))
            .filter(tag::Column::Digest.eq(digest.to_string()))
            .exec(&txn)
            .await?;

        // Delete referrer entries where this manifest is the referrer.
        referrer::Entity::delete_many()
            .filter(referrer::Column::RepoId.eq(repo_id))
            .filter(referrer::Column::ReferrerDigest.eq(digest.to_string()))
            .exec(&txn)
            .await?;

        // Delete manifest row.
        let res = manifest::Entity::delete_many()
            .filter(manifest::Column::RepoId.eq(repo_id))
            .filter(manifest::Column::Digest.eq(digest.to_string()))
            .exec(&txn)
            .await?;

        if res.rows_affected == 0 {
            txn.rollback().await?;
            return Err(OciError::manifest_unknown(digest.to_string()).into());
        }

        txn.commit().await?;
        Ok(())
    }

    pub async fn head_manifest(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        reference: &Reference,
    ) -> Result<(String, Digest, i64), RegistryError> {
        let repo_id = self.require_repo(tenant_id, name).await?;
        let digest = self.resolve_digest(repo_id, reference).await?;

        let row = manifest::Entity::find()
            .filter(manifest::Column::RepoId.eq(repo_id))
            .filter(manifest::Column::Digest.eq(digest.to_string()))
            .one(&self.db)
            .await?
            .ok_or_else(|| OciError::manifest_unknown(reference.to_string()))?;

        Ok((row.media_type, digest, row.size))
    }

    // -----------------------------------------------------------------------
    // Upload operations
    // -----------------------------------------------------------------------

    pub async fn start_upload(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
    ) -> Result<UploadId, RegistryError> {
        let repo_id = self.ensure_repository(tenant_id, name).await?;
        let upload_id = UploadId::new();
        let now = chrono::Utc::now().to_rfc3339();

        self.storage.create_upload(&upload_id).await?;

        let model = upload_session::ActiveModel {
            id: Set(upload_id.to_string()),
            repo_id: Set(repo_id),
            offset: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        };
        model.insert(&self.db).await?;

        Ok(upload_id)
    }

    /// Look up and verify an upload session belongs to the given repository.
    async fn require_upload(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        id: &UploadId,
    ) -> Result<upload_session::Model, RegistryError> {
        let repo_id = self.require_repo(tenant_id, name).await?;

        let session = upload_session::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await?
            .ok_or_else(OciError::blob_upload_unknown)?;

        if session.repo_id != repo_id {
            return Err(OciError::blob_upload_unknown().into());
        }

        Ok(session)
    }

    pub async fn get_upload_status(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        id: &UploadId,
    ) -> Result<u64, RegistryError> {
        let session = self.require_upload(tenant_id, name, id).await?;
        #[allow(clippy::cast_sign_loss)]
        let offset = session.offset as u64;
        Ok(offset)
    }

    pub async fn write_upload_chunk(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        id: &UploadId,
        data: BoxAsyncRead,
        offset: u64,
    ) -> Result<u64, RegistryError> {
        let session = self.require_upload(tenant_id, name, id).await?;

        #[allow(clippy::cast_sign_loss)]
        let session_offset = session.offset as u64;
        if session_offset != offset {
            return Err(OciError::blob_upload_invalid(format!(
                "expected offset {}, got {offset}",
                session.offset
            ))
            .into());
        }

        let new_offset = self.storage.write_upload_chunk(id, data, offset).await?;

        let now = chrono::Utc::now().to_rfc3339();
        #[allow(clippy::cast_possible_wrap)]
        let new_offset_i64 = new_offset as i64;
        let update = upload_session::ActiveModel {
            id: Set(session.id),
            offset: Set(new_offset_i64),
            updated_at: Set(now),
            ..Default::default()
        };
        update.update(&self.db).await?;

        Ok(new_offset)
    }

    pub async fn complete_upload(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        id: &UploadId,
        digest: &Digest,
    ) -> Result<(), RegistryError> {
        let session = self.require_upload(tenant_id, name, id).await?;

        // Verify content integrity before finalizing.
        let reader = self.storage.get_upload_reader(id).await?;
        let computed = Self::compute_digest(reader).await?;
        if computed != *digest {
            return Err(OciError::digest_invalid(format!(
                "computed {computed}, expected {digest}"
            ))
            .into());
        }

        // Finalize in storage (rename temp -> blob).
        self.storage.complete_upload(id, digest).await?;

        let size = self.storage.blob_size(digest).await?;

        let txn = self.db.begin().await?;

        // Insert repo_blob_link.
        #[allow(clippy::cast_possible_wrap)]
        let size_i64 = size as i64;
        let link = repo_blob_link::ActiveModel {
            repo_id: Set(session.repo_id),
            digest: Set(digest.to_string()),
            size: Set(size_i64),
        };
        repo_blob_link::Entity::insert(link)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    repo_blob_link::Column::RepoId,
                    repo_blob_link::Column::Digest,
                ])
                .update_column(repo_blob_link::Column::Size)
                .to_owned(),
            )
            .exec(&txn)
            .await?;

        // Delete upload session.
        upload_session::Entity::delete_by_id(session.id)
            .exec(&txn)
            .await?;

        txn.commit().await?;
        Ok(())
    }

    pub async fn cancel_upload(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        id: &UploadId,
    ) -> Result<(), RegistryError> {
        let session = self.require_upload(tenant_id, name, id).await?;

        upload_session::Entity::delete_by_id(session.id)
            .exec(&self.db)
            .await?;

        self.storage.abort_upload(id).await?;

        Ok(())
    }

    pub async fn monolithic_upload(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        digest: &Digest,
        data: BoxAsyncRead,
    ) -> Result<(), RegistryError> {
        let repo_id = self.ensure_repository(tenant_id, name).await?;

        self.storage.put_blob(digest, data).await?;

        // Verify content integrity.
        let reader = self.storage.get_blob(digest).await?;
        let computed = Self::compute_digest(reader).await?;
        if computed != *digest {
            let _ = self.storage.delete_blob(digest).await;
            return Err(OciError::digest_invalid(format!(
                "computed {computed}, expected {digest}"
            ))
            .into());
        }

        let size = self.storage.blob_size(digest).await?;

        #[allow(clippy::cast_possible_wrap)]
        let size_i64 = size as i64;
        let link = repo_blob_link::ActiveModel {
            repo_id: Set(repo_id),
            digest: Set(digest.to_string()),
            size: Set(size_i64),
        };
        repo_blob_link::Entity::insert(link)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    repo_blob_link::Column::RepoId,
                    repo_blob_link::Column::Digest,
                ])
                .update_column(repo_blob_link::Column::Size)
                .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Tag operations
    // -----------------------------------------------------------------------

    pub async fn list_tags(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        n: Option<u64>,
        last: Option<&str>,
    ) -> Result<(Vec<String>, bool), RegistryError> {
        let repo_id = self.require_repo(tenant_id, name).await?;

        let limit = n.unwrap_or(100);
        // Fetch one extra to determine has_more.
        let mut query = tag::Entity::find()
            .filter(tag::Column::RepoId.eq(repo_id))
            .order_by_asc(tag::Column::Name)
            .limit(limit + 1);

        if let Some(last_tag) = last {
            query = query.filter(tag::Column::Name.gt(last_tag));
        }

        let rows = query.all(&self.db).await?;
        #[allow(clippy::cast_possible_truncation)]
        let has_more = rows.len() as u64 > limit;
        let tags: Vec<String> = rows
            .into_iter()
            .take(
                #[allow(clippy::cast_possible_truncation)]
                {
                    limit as usize
                },
            )
            .map(|r| r.name)
            .collect();

        Ok((tags, has_more))
    }

    // -----------------------------------------------------------------------
    // Catalog
    // -----------------------------------------------------------------------

    pub async fn list_repositories(
        &self,
        tenant_id: &TenantId,
        n: Option<u64>,
        last: Option<&str>,
    ) -> Result<(Vec<String>, bool), RegistryError> {
        let limit = n.unwrap_or(100);
        let mut query = repository::Entity::find()
            .filter(repository::Column::TenantId.eq(tenant_id.as_str()))
            .order_by_asc(repository::Column::Name)
            .limit(limit + 1);

        if let Some(last_name) = last {
            query = query.filter(repository::Column::Name.gt(last_name));
        }

        let rows = query.all(&self.db).await?;
        #[allow(clippy::cast_possible_truncation)]
        let has_more = rows.len() as u64 > limit;
        let names: Vec<String> = rows
            .into_iter()
            .take(
                #[allow(clippy::cast_possible_truncation)]
                {
                    limit as usize
                },
            )
            .map(|r| r.name)
            .collect();

        Ok((names, has_more))
    }

    // -----------------------------------------------------------------------
    // Referrer operations
    // -----------------------------------------------------------------------

    pub async fn list_referrers(
        &self,
        tenant_id: &TenantId,
        name: &RepoName,
        digest: &Digest,
        artifact_type_filter: Option<&str>,
    ) -> Result<Vec<referrer::Model>, RegistryError> {
        let Ok(repo_id) = self.require_repo(tenant_id, name).await else {
            return Ok(vec![]);
        };

        let mut query = referrer::Entity::find()
            .filter(referrer::Column::RepoId.eq(repo_id))
            .filter(referrer::Column::SubjectDigest.eq(digest.to_string()));

        if let Some(at) = artifact_type_filter {
            query = query.filter(referrer::Column::ArtifactType.eq(at));
        }

        let rows = query.all(&self.db).await?;
        Ok(rows)
    }

    // -----------------------------------------------------------------------
    // Garbage collection
    // -----------------------------------------------------------------------

    pub async fn start_gc(&self) -> Result<String, RegistryError> {
        let txn = self.db.begin().await?;

        // Atomic check: reject if any job is still running or pending.
        let active = gc_job::Entity::find()
            .filter(
                gc_job::Column::Status
                    .eq("running")
                    .or(gc_job::Column::Status.eq("pending")),
            )
            .one(&txn)
            .await?;
        if active.is_some() {
            return Err(OciError::new(ErrorCode::Denied)
                .with_message("GC job already running")
                .into());
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let model = gc_job::ActiveModel {
            id: Set(job_id.clone()),
            status: Set("pending".to_owned()),
            started_at: Set(None),
            completed_at: Set(None),
            stats: Set(None),
            created_at: Set(now),
        };
        model.insert(&txn).await?;

        txn.commit().await?;
        Ok(job_id)
    }

    pub async fn run_gc(&self, job_id: &str) -> Result<(), RegistryError> {
        // Mark as running
        let now = chrono::Utc::now().to_rfc3339();
        gc_job::Entity::update_many()
            .col_expr(gc_job::Column::Status, Expr::value("running"))
            .col_expr(gc_job::Column::StartedAt, Expr::value(now))
            .filter(gc_job::Column::Id.eq(job_id))
            .exec(&self.db)
            .await?;

        let blobs_removed: u64 = 0;
        let manifests_removed: u64 = 0;
        let bytes_freed: u64 = 0;

        // Phase 1: Find orphaned manifests (in storage but not in DB manifests table).
        // For now, we skip this and rely on the manifest table being authoritative.

        // Phase 2: Find unreferenced blobs — blobs in storage not referenced by any
        // repo_blob_link. Uses a DB-driven approach; physical orphan cleanup (comparing
        // filesystem listing against DB digests) is a future enhancement.
        let _referenced: Vec<String> = repo_blob_link::Entity::find()
            .select_only()
            .column(repo_blob_link::Column::Digest)
            .distinct()
            .into_tuple()
            .all(&self.db)
            .await?;

        // Phase 3: Clean up expired upload sessions (older than 24 hours)
        let cutoff = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
        let expired_sessions: Vec<upload_session::Model> = upload_session::Entity::find()
            .filter(upload_session::Column::UpdatedAt.lt(&cutoff))
            .all(&self.db)
            .await?;

        for session in &expired_sessions {
            let upload_id: UploadId = session.id.parse().unwrap_or_else(|_| UploadId::new());
            let _ = self.storage.abort_upload(&upload_id).await;
            upload_session::Entity::delete_by_id(&session.id)
                .exec(&self.db)
                .await?;
        }

        // Phase 4: Delete unreferenced manifest storage files.
        // Manifests are only soft-deleted through the API for now; physical cleanup
        // for deleted manifests is a future enhancement.

        // Mark completed
        let now = chrono::Utc::now().to_rfc3339();
        let stats = serde_json::json!({
            "blobs_removed": blobs_removed,
            "manifests_removed": manifests_removed,
            "bytes_freed": bytes_freed,
            "expired_uploads_cleaned": expired_sessions.len(),
        });
        gc_job::Entity::update_many()
            .col_expr(gc_job::Column::Status, Expr::value("completed"))
            .col_expr(gc_job::Column::CompletedAt, Expr::value(now))
            .col_expr(gc_job::Column::Stats, Expr::value(stats.to_string()))
            .filter(gc_job::Column::Id.eq(job_id))
            .exec(&self.db)
            .await?;

        Ok(())
    }

    pub async fn mark_gc_failed(&self, job_id: &str) -> Result<(), RegistryError> {
        let now = chrono::Utc::now().to_rfc3339();
        gc_job::Entity::update_many()
            .col_expr(gc_job::Column::Status, Expr::value("failed"))
            .col_expr(gc_job::Column::CompletedAt, Expr::value(now))
            .filter(gc_job::Column::Id.eq(job_id))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    pub async fn get_gc_job(&self, job_id: &str) -> Result<Option<gc_job::Model>, RegistryError> {
        let job = gc_job::Entity::find_by_id(job_id).one(&self.db).await?;
        Ok(job)
    }
}
