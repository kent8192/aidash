//! Drive storage adapter for the shared resumable uploads/chunks/commit flow.
use super::repository::{self as db, Drive, Grant, Provenance, Publication, col};
use crate::{
	Error, Result, apps::execution::repositories::capability_objects::PendingObject,
	authorization::access::Access, store::Store,
};
use aidash_application::{
	capabilities::multipart::{self, CHUNK_BYTES},
	drive::Change,
	ports::blob_store::{BlobReader, BlobStore},
};
use aidash_domain::{
	capabilities::{Digest, references::lifecycle::Chunk},
	drive::{self, BaseVersion, PublishBase},
};
use chrono::{DateTime, Duration, Utc};
use reinhardt::query::{Alias, Expr, ExprTrait, LockType, Order};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "DriveUploadSpec")]
pub struct UploadSpec {
	pub idempotency_key: Uuid,
	pub path: String,
	pub size: i64,
	pub digest: String,
	pub base_version: BaseVersion,
	pub index: bool,
}

#[derive(Serialize, JsonSchema)]
#[schemars(rename = "DriveUploadReceipt")]
pub struct UploadReceipt {
	pub id: Uuid,
	pub uploaded_bytes: u64,
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

fn expires_at() -> DateTime<Utc> {
	Utc::now() + Duration::days(1)
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
	if spec.size < 0 || spec.digest.parse::<Digest>().is_err() {
		return Err(Error::Invalid("invalid upload size or digest".into()));
	}
	let key = format!("{}:{}", grant.person, spec.idempotency_key);
	let id = Uuid::new_v5(&drive.id, key.as_bytes());
	if let Some(upload) = find(access, drive, grant, id).await? {
		if upload.spec != spec {
			return Err(Error::Conflict("IDEMPOTENCY_CONFLICT".into()));
		}
		let uploaded_bytes = if upload.published_version.is_some() {
			spec.size as u64
		} else {
			progress(access, grant, id).await?
		};
		return Ok(UploadReceipt { id, uploaded_bytes });
	}
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
			expires_at().into(),
		],
	)
	.await?;
	Ok(UploadReceipt {
		id,
		uploaded_bytes: 0,
	})
}

async fn find(
	access: &mut Access,
	drive: &Drive,
	grant: &Grant,
	id: Uuid,
) -> Result<Option<Upload>> {
	let mut query = db::select("drive_uploads", &grant.tenant);
	query
		.and_where(col("id").eq(Expr::value(id)))
		.and_where(col("drive_id").eq(Expr::value(drive.id)))
		.and_where(col("author").eq(Expr::value(&grant.person)))
		.lock(LockType::Update);
	let upload = db::rows::<Upload>(access.tx.as_mut(), &query).await?.pop();
	if let Some(upload) = &upload {
		if upload.expires_at <= Utc::now() {
			return Err(Error::Conflict("UPLOAD_INCOMPLETE_OR_EXPIRED".into()));
		}
		if !grant.authority().writes(&upload.spec.path) {
			return Err(Error::Forbidden);
		}
	}
	Ok(upload)
}

async fn load(access: &mut Access, drive: &Drive, grant: &Grant, id: Uuid) -> Result<Upload> {
	find(access, drive, grant, id)
		.await?
		.ok_or_else(|| Error::NotFound("upload does not exist".into()))
}

fn part_query(grant: &Grant, id: Uuid) -> reinhardt::query::SelectStatement {
	db::select("drive_upload_parts", &grant.tenant)
		.and_where(col("upload_id").eq(Expr::value(id)))
		.to_owned()
}

async fn progress(access: &mut Access, grant: &Grant, id: Uuid) -> Result<u64> {
	let query = part_query(grant, id)
		.order_by(Alias::new("number"), Order::Desc)
		.limit(1)
		.to_owned();
	Ok(db::rows::<Part>(access.tx.as_mut(), &query)
		.await?
		.pop()
		.map_or(0, |part| {
			part.number as u64 * CHUNK_BYTES + part.size as u64
		}))
}

pub(super) async fn put_part(
	store: &Store,
	mut access: Access,
	drive: &Drive,
	grant: &Grant,
	id: Uuid,
	input: Chunk,
) -> Result<UploadReceipt> {
	let mut sealed = None;
	let result = async {
		let upload = load(&mut access, drive, grant, id).await?;
		if upload.published_version.is_some() {
			return Err(Error::Conflict("UPLOAD_UNAVAILABLE".into()));
		}
		if !input.offset.is_multiple_of(CHUNK_BYTES) {
			return Err(Error::Conflict("UPLOAD_OFFSET_CHANGED".into()));
		}
		let number = (input.offset / CHUNK_BYTES) as i64;
		let query = part_query(grant, id)
			.and_where(col("number").eq(Expr::value(number)))
			.to_owned();
		let previous = db::rows::<Part>(access.tx.as_mut(), &query).await?.pop();
		let prior = previous
			.as_ref()
			.map(|part| (part.digest.as_str(), part.size as u64));
		let uploaded_bytes = progress(&mut access, grant, id).await?;
		let Some(bytes) =
			multipart::next_chunk(&input, upload.spec.size as u64, uploaded_bytes, prior)?
		else {
			return Ok(UploadReceipt { id, uploaded_bytes });
		};
		let mut pending = store
			.capabilities
			.begin_object_for(
				access.tx.native_mut(),
				&grant.tenant,
				None,
				"drive",
				bytes.len() as u64,
				true,
			)
			.await?;
		pending.write_block(&bytes).await?;
		let object = sealed.insert(pending.seal(None).await?);
		db::insert(
			access.tx.as_mut(),
			"drive_upload_parts",
			["id", "tenant", "upload_id", "number", "digest", "size"],
			[
				Uuid::new_v4().into(),
				grant.tenant.clone().into(),
				id.into(),
				number.into(),
				object.digest().to_owned().into(),
				(bytes.len() as i64).into(),
			],
		)
		.await?;
		db::update(
			access.tx.as_mut(),
			"drive_uploads",
			&grant.tenant,
			id,
			[("expires_at", Expr::value(expires_at()).into())],
		)
		.await?;
		Ok(UploadReceipt {
			id,
			uploaded_bytes: input.offset + bytes.len() as u64,
		})
	}
	.await;
	// Keep reservations until the part references are committed.
	let result = access.finish(result).await;
	drop(sealed);
	result
}

struct DriveAssembly<'a> {
	blobs: &'a dyn BlobStore,
	tenant: &'a str,
	pending: &'a mut PendingObject,
}

#[async_trait::async_trait]
impl multipart::Assembly for DriveAssembly<'_> {
	type Part = Part;

	async fn read_part(&mut self, part: Part) -> aidash_application::Result<Vec<u8>> {
		let unavailable = || aidash_application::Error::Conflict("UPLOAD_PART_UNAVAILABLE".into());
		let digest = part.digest.parse().map_err(|_| unavailable())?;
		let stream = self
			.blobs
			.get(self.tenant, digest, None)
			.await?
			.ok_or_else(unavailable)?;
		let mut bytes = Vec::new();
		stream
			.take(CHUNK_BYTES + 1)
			.read_to_end(&mut bytes)
			.await
			.map_err(|error| aidash_application::Error::External(error.to_string()))?;
		if bytes.len() as i64 != part.size {
			return Err(unavailable());
		}
		Ok(bytes)
	}

	async fn append(&mut self, bytes: &[u8]) -> aidash_application::Result<()> {
		self.pending.write_block(bytes).await.map_err(Into::into)
	}
}

pub(super) async fn commit(
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
		if progress(&mut access, grant, id).await? != upload.spec.size as u64 {
			return Err(Error::Conflict("UPLOAD_INCOMPLETE_OR_EXPIRED".into()));
		}
		let query = part_query(grant, id)
			.order_by(Alias::new("number"), Order::Asc)
			.to_owned();
		let parts = db::rows::<Part>(access.tx.as_mut(), &query).await?;
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
		multipart::assemble(
			&mut DriveAssembly {
				blobs: blobs.as_ref(),
				tenant: &grant.tenant,
				pending: &mut pending,
			},
			parts,
		)
		.await?;
		let object = sealed.insert(pending.seal(Some(&upload.spec.digest)).await?);
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
