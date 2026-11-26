//! Durable multipart upload receipts and bounded streaming downloads.
use super::repository::{self as db, Drive, Grant, Provenance, Publication, col};
use crate::{Error, Result, authorization::access::Access, store::Store};
use aidash_application::{drive::Change, ports::blob_store::BlobReader};
use aidash_domain::drive::{self, BaseVersion, PublishBase};
use chrono::{DateTime, Duration, Utc};
use reinhardt::query::{Alias, Expr, ExprTrait, LockType, Order};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

pub(super) const PART_BYTES: usize = 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadSpec {
	pub path: String,
	pub size: i64,
	pub base_version: BaseVersion,
	pub index: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct UploadReceipt {
	pub id: Uuid,
	pub part_bytes: usize,
}

#[derive(Deserialize)]
struct Upload {
	id: Uuid,
	spec: UploadSpec,
	expires_at: DateTime<Utc>,
	published_version: Option<i64>,
}

#[derive(Deserialize)]
struct Part {
	number: i64,
	digest: String,
	size: i64,
}

pub(super) async fn start(
	access: &mut Access,
	drive: &Drive,
	grant: &Grant,
	mut spec: UploadSpec,
) -> Result<UploadReceipt> {
	spec.path = drive::path(&spec.path)?.to_owned();
	if !grant.authority().writes(&spec.path) {
		return Err(Error::Forbidden);
	}
	if spec.size < 0 {
		return Err(Error::Invalid("invalid upload size".into()));
	}
	let id = Uuid::new_v4();
	db::insert(
		access.tx.as_mut(),
		"drive_uploads",
		["id", "tenant", "drive_id", "author", "spec", "expires_at"],
		[
			id.into(),
			grant.tenant.clone().into(),
			drive.id.into(),
			grant.person.clone().into(),
			serde_json::to_value(spec)?.into(),
			(Utc::now() + Duration::days(1)).into(),
		],
	)
	.await?;
	Ok(UploadReceipt {
		id,
		part_bytes: PART_BYTES,
	})
}

async fn load(access: &mut Access, drive: &Drive, grant: &Grant, id: Uuid) -> Result<Upload> {
	let mut query = db::select("drive_uploads", &grant.tenant);
	query
		.and_where(col("id").eq(Expr::value(id)))
		.and_where(col("drive_id").eq(Expr::value(drive.id)))
		.and_where(col("author").eq(Expr::value(&grant.person)))
		.lock(LockType::Update);
	let upload = db::rows::<Upload>(access.tx.as_mut(), &query)
		.await?
		.pop()
		.filter(|upload| upload.expires_at > Utc::now())
		.ok_or_else(|| Error::NotFound("upload expired or does not exist".into()))?;
	if !grant.authority().writes(&upload.spec.path) {
		return Err(Error::Forbidden);
	}
	Ok(upload)
}

pub(super) async fn put_part(
	store: &Store,
	mut access: Access,
	drive: &Drive,
	grant: &Grant,
	id: Uuid,
	number: i64,
	bytes: &[u8],
) -> Result<UploadReceipt> {
	let mut sealed = None;
	let result = async {
		let upload = load(&mut access, drive, grant, id).await?;
		if upload.published_version.is_some() {
			return Err(Error::Conflict("upload is already published".into()));
		}
		let offset = number
			.checked_mul(PART_BYTES as i64)
			.filter(|offset| (0..upload.spec.size).contains(offset))
			.ok_or_else(|| Error::Invalid("invalid upload part number".into()))?;
		let expected = (upload.spec.size - offset).min(PART_BYTES as i64);
		if bytes.len() as i64 != expected {
			return Err(Error::Invalid("invalid upload part size".into()));
		}
		let digest = format!("{:x}", Sha256::digest(bytes));
		let mut query = db::select("drive_upload_parts", &grant.tenant);
		query
			.and_where(col("upload_id").eq(Expr::value(id)))
			.and_where(col("number").eq(Expr::value(number)));
		if let Some(part) = db::rows::<Part>(access.tx.as_mut(), &query).await?.pop() {
			if digest != part.digest {
				return Err(Error::Conflict("part already has different content".into()));
			}
			return Ok(UploadReceipt {
				id,
				part_bytes: PART_BYTES,
			});
		}
		let mut pending = store
			.capabilities
			.begin_object_for(
				access.tx.native_mut(),
				&grant.tenant,
				None,
				"drive",
				expected as u64,
				true,
			)
			.await?;
		pending.write_block(bytes).await?;
		sealed = Some(pending.seal(Some(&digest)).await?);
		db::insert(
			access.tx.as_mut(),
			"drive_upload_parts",
			["id", "tenant", "upload_id", "number", "digest", "size"],
			[
				Uuid::new_v4().into(),
				grant.tenant.clone().into(),
				id.into(),
				number.into(),
				digest.into(),
				expected.into(),
			],
		)
		.await?;
		Ok(UploadReceipt {
			id,
			part_bytes: PART_BYTES,
		})
	}
	.await;
	// Keep the blob reservation until the foreign key has committed.
	let result = access.finish(result).await;
	drop(sealed);
	result
}

pub(super) async fn finish(
	store: &Store,
	mut access: Access,
	drive: &Drive,
	grant: &Grant,
	id: Uuid,
) -> Result<Publication> {
	let mut sealed = None;
	let result = async {
		let upload = load(&mut access, drive, grant, id).await?;
		let mut changes = Vec::new();
		let path = upload.spec.path;
		let base = PublishBase::Paths(BTreeMap::from([(path.clone(), upload.spec.base_version)]));
		let provenance = Provenance {
			author: &grant.person,
			requester: &grant.person,
			task_id: None,
			run_id: None,
			operation_id: Some(upload.id),
		};
		if upload.published_version.is_some() {
			return db::publish(access.tx.as_mut(), drive, grant, &base, changes, provenance).await;
		}
		let query = db::select("drive_upload_parts", &grant.tenant)
			.and_where(col("upload_id").eq(Expr::value(id)))
			.order_by(Alias::new("number"), Order::Asc)
			.to_owned();
		let parts = db::rows::<Part>(access.tx.as_mut(), &query).await?;
		if parts
			.iter()
			.enumerate()
			.any(|(number, part)| part.number != number as i64)
			|| parts.iter().map(|part| part.size).sum::<i64>() != upload.spec.size
		{
			return Err(Error::Conflict("upload parts are incomplete".into()));
		}
		let blobs = store.capabilities.blob_store().await?;
		let mut pending = store
			.capabilities
			.begin_object_for(
				access.tx.native_mut(),
				&grant.tenant,
				None,
				"drive",
				upload.spec.size as u64,
				true,
			)
			.await?;
		let mut buffer = vec![0; 128 * 1024];
		for part in parts {
			let digest = part
				.digest
				.parse()
				.map_err(|_| Error::Conflict("OBJECT_INTEGRITY".into()))?;
			let Some(mut stream) = blobs.get(&grant.tenant, digest, None).await? else {
				return Err(Error::Conflict("upload part bytes are missing".into()));
			};
			loop {
				let count = stream.read(&mut buffer).await?;
				if count == 0 {
					break;
				}
				pending.write_block(&buffer[..count]).await?;
			}
		}
		let object = sealed.insert(pending.seal(None).await?);
		let media_type = mime_guess::from_path(&path)
			.first_or_octet_stream()
			.to_string();
		changes.push(Change::Put {
			path,
			digest: object.digest().into(),
			size: upload.spec.size,
			media_type,
			index: upload.spec.index,
		});
		let result =
			db::publish(access.tx.as_mut(), drive, grant, &base, changes, provenance).await?;
		if let Publication::Published { version } = &result {
			db::update(
				access.tx.as_mut(),
				"drive_uploads",
				&grant.tenant,
				id,
				[("published_version", Expr::value(version.version).into())],
			)
			.await?;
			db::delete(
				access.tx.as_mut(),
				"drive_upload_parts",
				&grant.tenant,
				col("upload_id").eq(Expr::value(id)),
			)
			.await?;
		}
		Ok(result)
	}
	.await;
	let result = access.finish(result).await;
	drop(sealed);
	result
}

pub(super) async fn open(
	store: &Store,
	grant: &Grant,
	entry: &db::Entry,
	range: Option<std::ops::Range<u64>>,
) -> Result<BlobReader> {
	let digest = entry
		.file
		.digest
		.parse()
		.map_err(|_| Error::Conflict("OBJECT_INTEGRITY".into()))?;
	store
		.capabilities
		.blob_store()
		.await?
		.get(&grant.tenant, digest, range)
		.await?
		.ok_or_else(|| Error::NotFound("Drive file bytes not found".into()))
}

/// An HTTP single range, normalized to the BlobStore's exclusive end.
pub(super) fn byte_range(value: &str, size: u64) -> Option<std::ops::Range<u64>> {
	let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
	if start.is_empty() {
		let suffix = end.parse::<u64>().ok()?;
		return (size > 0 && suffix > 0).then(|| size.saturating_sub(suffix)..size);
	}
	let start = start.parse::<u64>().ok()?;
	let end = if end.is_empty() {
		size
	} else {
		end.parse::<u64>().ok()?.saturating_add(1).min(size)
	};
	(start < end).then_some(start..end)
}

#[cfg(test)]
mod tests {
	use super::byte_range;

	#[test]
	fn resumable_downloads_use_http_inclusive_end_and_clip_to_file_size() {
		assert_eq!(byte_range("bytes=8-9", 12), Some(8..10));
		assert_eq!(byte_range("bytes=10-99", 12), Some(10..12));
		assert_eq!(byte_range("bytes=8-", 12), Some(8..12));
		assert_eq!(byte_range("bytes=-4", 12), Some(8..12));
		assert_eq!(byte_range("bytes=12-", 12), None);
		assert_eq!(byte_range("bytes=8-7", 12), None);
	}
}
