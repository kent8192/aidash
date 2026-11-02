//! Tenant-isolated, immutable content-addressed blob storage.
use std::{ops::Range, pin::Pin};

use aidash_domain::capabilities::Digest;
use async_trait::async_trait;
use tokio::io::AsyncRead;

use crate::Result;

pub type BlobReader = Pin<Box<dyn AsyncRead + Send + Unpin>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutResult {
	Inserted,
	AlreadyPresent,
}

/// Callers authorize access and coordinate publication and deletion with references.
/// Implementations verify size and SHA-256 before atomically exposing a new blob;
/// a concurrent writer must never overwrite an existing blob. Range ends are exclusive.
#[async_trait]
pub trait BlobStore: Send + Sync {
	async fn put_if_absent(
		&self,
		tenant: &str,
		digest: Digest,
		size: u64,
		input: BlobReader,
	) -> Result<PutResult>;
	async fn get(
		&self,
		tenant: &str,
		digest: Digest,
		range: Option<Range<u64>>,
	) -> Result<Option<BlobReader>>;
	async fn exists(&self, tenant: &str, digest: Digest) -> Result<bool>;
	async fn delete(&self, tenant: &str, digest: Digest) -> Result<()>;
	/// Remove all committed and temporary objects belonging to a Tenant.
	async fn delete_tenant(&self, tenant: &str) -> Result<()>;
}

#[cfg(any(test, feature = "test-support"))]
pub mod memory;
