//! In-memory implementation for consumers' tests.
use std::{collections::HashMap, ops::Range, sync::Mutex};

use aidash_domain::capabilities::Digest;
use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt;

use super::{BlobReader, BlobStore, PutResult};
use crate::{Error, Result};

#[derive(Default)]
pub struct MemoryBlobStore {
	objects: Mutex<HashMap<(String, Digest), Vec<u8>>>,
}

#[async_trait]
impl BlobStore for MemoryBlobStore {
	async fn put_if_absent(
		&self,
		tenant: &str,
		digest: Digest,
		size: u64,
		mut input: BlobReader,
	) -> Result<PutResult> {
		if let Some(existing) = self
			.objects
			.lock()
			.map_err(|_| Error::External("blob lock poisoned".into()))?
			.get(&(tenant.to_owned(), digest))
		{
			return if existing.len() as u64 == size {
				Ok(PutResult::AlreadyPresent)
			} else {
				Err(Error::Conflict("OBJECT_INTEGRITY".into()))
			};
		}
		let mut bytes = Vec::new();
		input
			.read_to_end(&mut bytes)
			.await
			.map_err(|e| Error::External(e.to_string()))?;
		if bytes.len() as u64 != size || Sha256::digest(&bytes).as_slice() != digest.as_bytes() {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		let mut objects = self
			.objects
			.lock()
			.map_err(|_| Error::External("blob lock poisoned".into()))?;
		use std::collections::hash_map::Entry;
		match objects.entry((tenant.to_owned(), digest)) {
			Entry::Occupied(_) => Ok(PutResult::AlreadyPresent),
			Entry::Vacant(entry) => {
				entry.insert(bytes);
				Ok(PutResult::Inserted)
			}
		}
	}

	async fn get(
		&self,
		tenant: &str,
		digest: Digest,
		range: Option<Range<u64>>,
	) -> Result<Option<BlobReader>> {
		let objects = self
			.objects
			.lock()
			.map_err(|_| Error::External("blob lock poisoned".into()))?;
		let Some(bytes) = objects.get(&(tenant.to_owned(), digest)) else {
			return Ok(None);
		};
		let bytes = match range {
			Some(range) => {
				let start = usize::try_from(range.start)
					.map_err(|_| Error::Invalid("invalid blob range".into()))?;
				let end = usize::try_from(range.end)
					.map_err(|_| Error::Invalid("invalid blob range".into()))?;
				bytes
					.get(start..end)
					.ok_or_else(|| Error::Invalid("invalid blob range".into()))?
					.to_vec()
			}
			None => bytes.clone(),
		};
		Ok(Some(Box::pin(std::io::Cursor::new(bytes))))
	}
	async fn exists(&self, tenant: &str, digest: Digest) -> Result<bool> {
		Ok(self
			.objects
			.lock()
			.map_err(|_| Error::External("blob lock poisoned".into()))?
			.contains_key(&(tenant.to_owned(), digest)))
	}
	async fn delete(&self, tenant: &str, digest: Digest) -> Result<()> {
		self.objects
			.lock()
			.map_err(|_| Error::External("blob lock poisoned".into()))?
			.remove(&(tenant.to_owned(), digest));
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn existing_blob_is_reused_without_consuming_the_input() {
		let store = MemoryBlobStore::default();
		let digest: Digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
			.parse()
			.unwrap();
		store
			.put_if_absent("one", digest, 3, Box::pin(std::io::Cursor::new(b"abc")))
			.await
			.unwrap();
		let result = store
			.put_if_absent("one", digest, 3, Box::pin(std::io::Cursor::new(b"bad")))
			.await
			.unwrap();
		assert_eq!(result, PutResult::AlreadyPresent);
		assert!(
			store
				.put_if_absent("one", digest, 4, Box::pin(std::io::Cursor::new(b"")))
				.await
				.is_err()
		);
	}

	#[tokio::test]
	async fn isolates_tenants_and_rejects_incorrect_content() {
		let store = MemoryBlobStore::default();
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
		assert_eq!(
			store
				.put_if_absent("one", digest, 3, Box::pin(std::io::Cursor::new(b"abc")))
				.await
				.unwrap(),
			PutResult::Inserted
		);
		assert!(!store.exists("two", digest).await.unwrap());
		let mut reader = store.get("one", digest, Some(1..3)).await.unwrap().unwrap();
		let mut data = Vec::new();
		reader.read_to_end(&mut data).await.unwrap();
		assert_eq!(data, b"bc");
		store.delete("one", digest).await.unwrap();
		assert!(!store.exists("one", digest).await.unwrap());
	}
}
