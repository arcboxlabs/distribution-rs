use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

use crate::types::{Digest, UploadId};

use super::{BoxAsyncRead, Storage};

pub struct FilesystemStorage {
    root_dir: PathBuf,
}

impl FilesystemStorage {
    pub async fn new(root_dir: PathBuf) -> Result<Self> {
        for sub in ["blobs", "manifests", "uploads", "tmp"] {
            fs::create_dir_all(root_dir.join(sub))
                .await
                .with_context(|| {
                    format!("creating {sub} directory under {}", root_dir.display())
                })?;
        }
        Ok(Self { root_dir })
    }

    fn blob_path(&self, digest: &Digest) -> PathBuf {
        let hex = digest.hex();
        self.root_dir
            .join("blobs")
            .join(digest.algorithm().to_string())
            .join(&hex[..2])
            .join(hex)
    }

    fn manifest_path(&self, digest: &Digest) -> PathBuf {
        let hex = digest.hex();
        self.root_dir
            .join("manifests")
            .join(digest.algorithm().to_string())
            .join(&hex[..2])
            .join(hex)
    }

    fn upload_path(&self, id: &UploadId) -> PathBuf {
        self.root_dir.join("uploads").join(id.to_string())
    }

    fn tmp_path(&self) -> PathBuf {
        self.root_dir.join("tmp").join(Uuid::new_v4().to_string())
    }

    async fn atomic_write(&self, target: &Path, data: &[u8]) -> Result<()> {
        let tmp = self.tmp_path();
        ensure_parent_dir(&tmp).await?;

        let mut file = fs::File::create(&tmp)
            .await
            .with_context(|| format!("creating temp file {}", tmp.display()))?;
        file.write_all(data).await.context("writing temp file")?;
        file.sync_all().await.context("fsync temp file")?;
        drop(file);

        ensure_parent_dir(target).await?;

        if fs::metadata(target).await.is_ok() {
            // CAS dedup: target already exists, discard temp file.
            let _ = fs::remove_file(&tmp).await;
            return Ok(());
        }

        fs::rename(&tmp, target)
            .await
            .with_context(|| format!("renaming temp file to {}", target.display()))?;
        Ok(())
    }
}

async fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating parent directory {}", parent.display()))?;
    }
    Ok(())
}

#[async_trait]
impl Storage for FilesystemStorage {
    async fn blob_exists(&self, digest: &Digest) -> Result<bool> {
        Ok(fs::metadata(self.blob_path(digest)).await.is_ok())
    }

    async fn get_blob(&self, digest: &Digest) -> Result<BoxAsyncRead> {
        let path = self.blob_path(digest);
        let file = fs::File::open(&path)
            .await
            .with_context(|| format!("opening blob {}", path.display()))?;
        Ok(Box::new(file))
    }

    async fn get_blob_range(
        &self,
        digest: &Digest,
        offset: u64,
        length: u64,
    ) -> Result<BoxAsyncRead> {
        let path = self.blob_path(digest);
        let mut file = fs::File::open(&path)
            .await
            .with_context(|| format!("opening blob {}", path.display()))?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .with_context(|| format!("seeking blob {} to offset {offset}", path.display()))?;
        Ok(Box::new(file.take(length)))
    }

    async fn put_blob(&self, digest: &Digest, mut data: BoxAsyncRead) -> Result<()> {
        let target = self.blob_path(digest);

        // If already present (CAS dedup), skip.
        if fs::metadata(&target).await.is_ok() {
            return Ok(());
        }

        let tmp = self.tmp_path();
        ensure_parent_dir(&tmp).await?;

        let mut file = fs::File::create(&tmp)
            .await
            .with_context(|| format!("creating temp file {}", tmp.display()))?;
        tokio::io::copy(&mut data, &mut file)
            .await
            .context("streaming blob to temp file")?;
        file.sync_all().await.context("fsync blob temp file")?;
        drop(file);

        ensure_parent_dir(&target).await?;

        // Re-check after write; another writer may have raced us.
        if fs::metadata(&target).await.is_ok() {
            let _ = fs::remove_file(&tmp).await;
            return Ok(());
        }

        fs::rename(&tmp, &target)
            .await
            .with_context(|| format!("renaming blob to {}", target.display()))?;
        Ok(())
    }

    async fn delete_blob(&self, digest: &Digest) -> Result<()> {
        let path = self.blob_path(digest);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("deleting blob {}", path.display())),
        }
    }

    async fn blob_size(&self, digest: &Digest) -> Result<u64> {
        let path = self.blob_path(digest);
        let meta = fs::metadata(&path)
            .await
            .with_context(|| format!("stat blob {}", path.display()))?;
        Ok(meta.len())
    }

    async fn get_manifest_bytes(&self, digest: &Digest) -> Result<Vec<u8>> {
        let path = self.manifest_path(digest);
        fs::read(&path)
            .await
            .with_context(|| format!("reading manifest {}", path.display()))
    }

    async fn put_manifest_bytes(&self, digest: &Digest, data: &[u8]) -> Result<()> {
        let target = self.manifest_path(digest);
        self.atomic_write(&target, data).await
    }

    async fn delete_manifest_bytes(&self, digest: &Digest) -> Result<()> {
        let path = self.manifest_path(digest);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("deleting manifest {}", path.display())),
        }
    }

    async fn create_upload(&self, id: &UploadId) -> Result<()> {
        let path = self.upload_path(id);
        ensure_parent_dir(&path).await?;
        fs::File::create(&path)
            .await
            .with_context(|| format!("creating upload file {}", path.display()))?;
        Ok(())
    }

    async fn write_upload_chunk(
        &self,
        id: &UploadId,
        mut data: BoxAsyncRead,
        offset: u64,
    ) -> Result<u64> {
        let path = self.upload_path(id);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .with_context(|| format!("opening upload file {}", path.display()))?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .with_context(|| format!("seeking upload {} to offset {offset}", path.display()))?;
        let written = tokio::io::copy(&mut data, &mut file)
            .await
            .context("writing upload chunk")?;
        Ok(offset + written)
    }

    async fn get_upload_reader(&self, id: &UploadId) -> Result<BoxAsyncRead> {
        let path = self.upload_path(id);
        let file = fs::File::open(&path)
            .await
            .with_context(|| format!("opening upload file {}", path.display()))?;
        Ok(Box::new(file))
    }

    async fn complete_upload(&self, id: &UploadId, digest: &Digest) -> Result<()> {
        let src = self.upload_path(id);
        let target = self.blob_path(digest);
        ensure_parent_dir(&target).await?;

        // Fsync before rename to ensure data durability.
        let file = fs::File::open(&src)
            .await
            .with_context(|| format!("opening upload file for fsync {}", src.display()))?;
        file.sync_all().await.context("fsync upload file")?;
        drop(file);

        fs::rename(&src, &target)
            .await
            .with_context(|| format!("renaming upload to blob {}", target.display()))?;
        Ok(())
    }

    async fn abort_upload(&self, id: &UploadId) -> Result<()> {
        let path = self.upload_path(id);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("aborting upload {}", path.display())),
        }
    }

    async fn list_upload_ids(&self) -> Result<Vec<String>> {
        let uploads_dir = self.root_dir.join("uploads");
        let mut ids = Vec::new();
        let mut entries = fs::read_dir(&uploads_dir)
            .await
            .with_context(|| format!("reading uploads dir {}", uploads_dir.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                ids.push(name.to_owned());
            }
        }
        Ok(ids)
    }

    async fn cleanup_tmp(&self) -> Result<u64> {
        let tmp_dir = self.root_dir.join("tmp");
        let mut removed: u64 = 0;
        let mut entries = fs::read_dir(&tmp_dir)
            .await
            .with_context(|| format!("reading tmp dir {}", tmp_dir.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            match fs::remove_file(&path).await {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!("failed to remove tmp file {}: {e}", path.display());
                }
            }
        }
        Ok(removed)
    }
}
