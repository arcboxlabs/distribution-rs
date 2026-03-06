pub mod filesystem;

use crate::types::{Digest, UploadId};
use anyhow::Result;
use async_trait::async_trait;
use tokio::io::AsyncRead;

pub type BoxAsyncRead = Box<dyn AsyncRead + Send + Unpin>;

#[async_trait]
pub trait Storage: Send + Sync {
    // Blob: stream in/out, never buffer entire blob in memory
    async fn blob_exists(&self, digest: &Digest) -> Result<bool>;
    async fn get_blob(&self, digest: &Digest) -> Result<BoxAsyncRead>;
    async fn get_blob_range(
        &self,
        digest: &Digest,
        offset: u64,
        length: u64,
    ) -> Result<BoxAsyncRead>;
    async fn put_blob(&self, digest: &Digest, data: BoxAsyncRead) -> Result<()>;
    async fn delete_blob(&self, digest: &Digest) -> Result<()>;
    async fn blob_size(&self, digest: &Digest) -> Result<u64>;

    // Manifest: small enough to buffer as Vec<u8>
    async fn get_manifest_bytes(&self, digest: &Digest) -> Result<Vec<u8>>;
    async fn put_manifest_bytes(&self, digest: &Digest, data: &[u8]) -> Result<()>;
    async fn delete_manifest_bytes(&self, digest: &Digest) -> Result<()>;

    // Upload session temp files (streaming)
    async fn create_upload(&self, id: &UploadId) -> Result<()>;
    /// Write a chunk at the given offset. Returns the next write offset (`offset + bytes_written`).
    async fn write_upload_chunk(
        &self,
        id: &UploadId,
        data: BoxAsyncRead,
        offset: u64,
    ) -> Result<u64>;
    /// Return a reader for the upload temp file (used for digest verification before finalize).
    async fn get_upload_reader(&self, id: &UploadId) -> Result<BoxAsyncRead>;
    async fn complete_upload(&self, id: &UploadId, digest: &Digest) -> Result<()>;
    async fn abort_upload(&self, id: &UploadId) -> Result<()>;

    // Startup cleanup
    /// List all upload IDs present in storage (for orphan detection).
    async fn list_upload_ids(&self) -> Result<Vec<String>>;
    /// Remove all files from the tmp directory.
    async fn cleanup_tmp(&self) -> Result<u64>;
}
