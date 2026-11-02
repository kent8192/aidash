//! Immutable Tenant-owned blobs backed by an object store.
use crate::{Error, Result};
use aidash_application::ports::blob_store::{BlobReader, BlobStore, PutResult};
use aidash_domain::capabilities::Digest;
use async_trait::async_trait;
use futures_util::TryStreamExt;
use object_store::{
	GetOptions, GetRange, ObjectStore, ObjectStoreExt, PutMode, PutOptions, WriteMultipart,
	path::Path as ObjectPath,
};
use sha2::{Digest as _, Sha256};
use std::{ops::Range, path::Path, sync::Arc};
use tokio::io::AsyncReadExt;

fn external(e: impl std::fmt::Display) -> Error {
	Error::External(e.to_string())
}

/// Backends must provide atomic conditional copy (S3 requires its multipart
/// copy-if-not-exists configuration). Temporary objects need lifecycle cleanup
/// for interrupted processes.
pub struct ObjectBlobStore {
	store: Arc<dyn ObjectStore>,
}
impl ObjectBlobStore {
	pub fn new(store: Arc<dyn ObjectStore>) -> Self {
		Self { store }
	}

	/// Uses standard AWS credential and endpoint environment configuration.
	pub fn s3(bucket: &str) -> Result<Self> {
		let store = object_store::aws::AmazonS3Builder::from_env()
			.with_bucket_name(bucket)
			.with_copy_if_not_exists(object_store::aws::S3CopyIfNotExists::Multipart)
			.build()
			.map_err(external)?;
		Ok(Self::new(Arc::new(store)))
	}

	/// Uses Google application-default credentials and standard configuration.
	pub fn gcs(bucket: &str) -> Result<Self> {
		let store = object_store::gcp::GoogleCloudStorageBuilder::from_env()
			.with_bucket_name(bucket)
			.build()
			.map_err(external)?;
		Ok(Self::new(Arc::new(store)))
	}
	fn path(tenant: &str, digest: Digest) -> ObjectPath {
		let tenant_hash = format!("{:x}", Sha256::digest(tenant.as_bytes()));
		let hex = digest.to_string();
		ObjectPath::from(format!(
			"tenants/{tenant_hash}/sha256/{}/{}/{hex}",
			&hex[..2],
			&hex[2..4]
		))
	}
}

#[async_trait]
impl BlobStore for ObjectBlobStore {
	async fn put_if_absent(
		&self,
		tenant: &str,
		digest: Digest,
		size: u64,
		mut input: BlobReader,
	) -> Result<PutResult> {
		let destination = Self::path(tenant, digest);
		match self.store.head(&destination).await {
			Ok(meta) if meta.size == size => return Ok(PutResult::AlreadyPresent),
			Ok(_) => return Err(Error::Conflict("OBJECT_INTEGRITY".into())),
			Err(object_store::Error::NotFound { .. }) => {}
			Err(error) => return Err(external(error)),
		}
		if size == 0 {
			let mut byte = [0; 1];
			if input.read(&mut byte).await.map_err(external)? != 0
				|| Sha256::digest([]).as_slice() != digest.as_bytes()
			{
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			return match self
				.store
				.put_opts(
					&destination,
					Vec::<u8>::new().into(),
					PutOptions {
						mode: PutMode::Create,
						..Default::default()
					},
				)
				.await
			{
				Ok(_) => Ok(PutResult::Inserted),
				Err(object_store::Error::AlreadyExists { .. }) => Ok(PutResult::AlreadyPresent),
				Err(e) => Err(external(e)),
			};
		}
		let tenant_hash = format!("{:x}", Sha256::digest(tenant.as_bytes()));
		let temp = ObjectPath::from(format!("temporary/{tenant_hash}/{}", uuid::Uuid::new_v4()));
		let upload = self.store.put_multipart(&temp).await.map_err(external)?;
		let mut upload = WriteMultipart::new(upload);
		let mut hash = Sha256::new();
		let mut count = 0u64;
		let mut buffer = vec![0; 64 * 1024];
		loop {
			let n = match input.read(&mut buffer).await {
				Ok(n) => n,
				Err(e) => {
					let _ = upload.abort().await;
					return Err(external(e));
				}
			};
			if n == 0 {
				break;
			}
			count = match count.checked_add(n as u64) {
				Some(count) if count <= size => count,
				_ => {
					let _ = upload.abort().await;
					return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
				}
			};
			hash.update(&buffer[..n]);
			if let Err(e) = upload.wait_for_capacity(2).await {
				let _ = upload.abort().await;
				return Err(external(e));
			}
			upload.write(&buffer[..n]);
		}
		if count != size || hash.finalize().as_slice() != digest.as_bytes() {
			let _ = upload.abort().await;
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		upload.finish().await.map_err(external)?;
		let copy = self.store.copy_if_not_exists(&temp, &destination).await;
		let cleanup = self.store.delete(&temp).await;
		let result = match copy {
			Ok(()) => Ok(PutResult::Inserted),
			Err(object_store::Error::AlreadyExists { .. }) => Ok(PutResult::AlreadyPresent),
			Err(e) => Err(external(e)),
		};
		cleanup.map_err(external)?;
		result
	}
	async fn get(
		&self,
		tenant: &str,
		digest: Digest,
		range: Option<Range<u64>>,
	) -> Result<Option<BlobReader>> {
		if range.as_ref().is_some_and(|r| r.start >= r.end) {
			return Err(Error::Invalid("invalid blob range".into()));
		}
		let options = GetOptions {
			range: range.map(GetRange::Bounded),
			..Default::default()
		};
		match self
			.store
			.get_opts(&Self::path(tenant, digest), options)
			.await
		{
			Ok(value) => {
				let stream = value.into_stream().map_err(std::io::Error::other);
				Ok(Some(Box::pin(tokio_util::io::StreamReader::new(stream))))
			}
			Err(object_store::Error::NotFound { .. }) => Ok(None),
			Err(e) => Err(external(e)),
		}
	}
	async fn exists(&self, tenant: &str, digest: Digest) -> Result<bool> {
		match self.store.head(&Self::path(tenant, digest)).await {
			Ok(_) => Ok(true),
			Err(object_store::Error::NotFound { .. }) => Ok(false),
			Err(e) => Err(external(e)),
		}
	}
	async fn delete(&self, tenant: &str, digest: Digest) -> Result<()> {
		match self.store.delete(&Self::path(tenant, digest)).await {
			Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
			Err(e) => Err(external(e)),
		}
	}
}

pub struct LocalBlobStore(ObjectBlobStore);
impl LocalBlobStore {
	pub fn new(root: &Path) -> Result<Self> {
		let store =
			object_store::local::LocalFileSystem::new_with_prefix(root).map_err(external)?;
		Ok(Self(ObjectBlobStore::new(Arc::new(store))))
	}
}
#[async_trait]
impl BlobStore for LocalBlobStore {
	async fn put_if_absent(
		&self,
		tenant: &str,
		digest: Digest,
		size: u64,
		input: BlobReader,
	) -> Result<PutResult> {
		self.0.put_if_absent(tenant, digest, size, input).await
	}
	async fn get(
		&self,
		tenant: &str,
		digest: Digest,
		range: Option<Range<u64>>,
	) -> Result<Option<BlobReader>> {
		self.0.get(tenant, digest, range).await
	}
	async fn exists(&self, tenant: &str, digest: Digest) -> Result<bool> {
		self.0.exists(tenant, digest).await
	}
	async fn delete(&self, tenant: &str, digest: Digest) -> Result<()> {
		self.0.delete(tenant, digest).await
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_application::ports::blob_store::{BlobStore, PutResult};
	use aidash_domain::capabilities::Digest;
	use std::sync::Arc;
	use tokio::io::AsyncReadExt;

	async fn contract(store: Arc<dyn BlobStore>) {
		let digest: Digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
			.parse()
			.unwrap();
		assert!(
			store
				.put_if_absent("one", digest, 3, Box::pin(std::io::Cursor::new(b"bad")))
				.await
				.is_err()
		);
		assert!(!store.exists("one", digest).await.unwrap());
		let empty: Digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
			.parse()
			.unwrap();
		assert_eq!(
			store
				.put_if_absent("one", empty, 0, Box::pin(std::io::Cursor::new(b"")))
				.await
				.unwrap(),
			PutResult::Inserted
		);
		assert!(store.exists("one", empty).await.unwrap());
		store.delete("one", empty).await.unwrap();
		let input = || Box::pin(std::io::Cursor::new(b"abc"));
		let (a, b) = tokio::join!(
			store.put_if_absent("one", digest, 3, input()),
			store.put_if_absent("one", digest, 3, input())
		);
		assert!(matches!(
			(a.unwrap(), b.unwrap()),
			(PutResult::Inserted, PutResult::AlreadyPresent)
				| (PutResult::AlreadyPresent, PutResult::Inserted)
		));
		assert!(
			store
				.put_if_absent("one", digest, 4, Box::pin(std::io::Cursor::new(b"")))
				.await
				.is_err()
		);
		assert!(!store.exists("two", digest).await.unwrap());
		let mut reader = store.get("one", digest, Some(1..3)).await.unwrap().unwrap();
		let mut output = vec![];
		reader.read_to_end(&mut output).await.unwrap();
		assert_eq!(output, b"bc");
		store.delete("one", digest).await.unwrap();
		assert!(!store.exists("one", digest).await.unwrap());
	}
	#[tokio::test]
	async fn disk_contract() {
		let dir = tempfile::tempdir().unwrap();
		contract(Arc::new(LocalBlobStore::new(dir.path()).unwrap())).await;
	}
	#[tokio::test]
	async fn cloud_contract() {
		contract(Arc::new(ObjectBlobStore::new(Arc::new(
			object_store::memory::InMemory::new(),
		))))
		.await;
	}
}
