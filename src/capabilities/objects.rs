//! Immutable node-managed objects. User paths never become host filesystem paths.
use super::{
	Runtime,
	contracts::{FileEntry, FileScope},
};
use crate::{Error, Result, authorization::access::Access};
use sea_orm::sea_query::{Alias, Expr, OnConflict, PostgresQueryBuilder, Query};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

pub(crate) struct PendingObject {
	id: Uuid,
	area: Option<Uuid>,
	kind: String,
	size: u64,
	written: u64,
	file: tokio::fs::File,
	hash: Sha256,
	root: PathBuf,
}

impl PendingObject {
	pub(crate) async fn write_block(&mut self, bytes: &[u8]) -> Result<()> {
		self.written = self
			.written
			.checked_add(bytes.len() as u64)
			.ok_or(Error::Forbidden)?;
		if self.written > self.size {
			return Err(Error::Conflict("OBJECT_SIZE_CHANGED".into()));
		}
		self.file.write_all(bytes).await?;
		self.hash.update(bytes);
		Ok(())
	}
	pub(crate) async fn finish(
		self,
		access: &mut Access,
		expected: Option<&str>,
	) -> Result<(Uuid, String)> {
		let hash = format!("{:x}", self.hash.finalize());
		if self.written != self.size || expected.is_some_and(|expected| hash != expected) {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		self.file.sync_all().await?;
		tokio::fs::File::open(&self.root).await?.sync_all().await?;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("core_objects"))
				.columns(["id", "tenant", "area_id", "kind", "digest", "size"].map(Alias::new))
				.values_panic((1..=6).map(|i| Expr::cust(format!("${i}"))))
				.to_string(PostgresQueryBuilder),
		)
		.bind(self.id)
		.bind(&access.identity.tenant)
		.bind(self.area)
		.bind(self.kind)
		.bind(&hash)
		.bind(self.size as i64)
		.execute(&mut **access.tx)
		.await?;
		Ok((self.id, hash))
	}
}

pub fn digest(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}
pub fn validate_path(path: &str) -> Result<()> {
	if path.is_empty()
		|| path.len() > 1024
		|| path.contains('\\')
		|| path.chars().any(char::is_control)
		|| path
			.split('/')
			.any(|p| p.is_empty() || matches!(p, "." | ".."))
	{
		return Err(Error::Invalid(
			"INVALID_PATH: expected a relative regular-file path".into(),
		));
	}
	Ok(())
}
impl Runtime {
	/// Reconcile crash/rollback debris without impersonating a user. A writer
	/// holds the same object lock until its database transaction commits. Once
	/// acquired, absence of the object row proves these bytes have no owner.
	pub(crate) async fn reconcile_orphan_batch(
		&self,
		pool: &sqlx::PgPool,
		cursor: &mut Option<tokio::fs::ReadDir>,
	) -> Result<()> {
		let intents = self.0.storage.join(".intents");
		if cursor.is_none() {
			*cursor = match tokio::fs::read_dir(&intents).await {
				Ok(entries) => Some(entries),
				Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
				Err(e) => return Err(e.into()),
			};
		}
		for _ in 0..32 {
			let Some(entry) = cursor.as_mut().unwrap().next_entry().await? else {
				*cursor = None;
				break;
			};
			let Some(id) = entry
				.file_name()
				.to_str()
				.and_then(|s| Uuid::parse_str(s).ok())
			else {
				continue;
			};
			let mut tx = pool.begin().await?;
			let acquired: bool = sqlx::query_scalar(
				&Query::select()
					.expr(Expr::cust(
						"pg_try_advisory_xact_lock(hashtextextended($1, 0))",
					))
					.to_string(PostgresQueryBuilder),
			)
			.bind(format!("core-object:{id}"))
			.fetch_one(&mut *tx)
			.await?;
			if !acquired {
				continue;
			}
			let mut options = tokio::fs::OpenOptions::new();
			options.read(true);
			#[cfg(unix)]
			options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
			let file = match options.open(entry.path()).await {
				Ok(file) => file,
				Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
				Err(e) => return Err(e.into()),
			};
			let metadata = file.metadata().await?;
			if !metadata.is_file() || metadata.len() > 8192 {
				return Err(Error::Conflict("unsafe object ownership intent".into()));
			}
			let mut bytes = vec![];
			file.take(8193).read_to_end(&mut bytes).await?;
			let intent: serde_json::Value = serde_json::from_slice(&bytes)?;
			if intent["id"] != id.to_string() || intent["tenant"].as_str().is_none_or(str::is_empty)
			{
				return Err(Error::Conflict("invalid object ownership intent".into()));
			}
			let exists: Option<Uuid> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_objects"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.to_string(PostgresQueryBuilder),
			)
			.bind(id)
			.fetch_optional(&mut *tx)
			.await?;
			if exists.is_none() {
				match tokio::fs::remove_file(self.object_path(id)).await {
					Ok(()) => {}
					Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
					Err(e) => return Err(e.into()),
				}
			}
			match tokio::fs::remove_file(entry.path()).await {
				Ok(()) => {}
				Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
				Err(e) => return Err(e.into()),
			}
			tx.commit().await?;
		}
		Ok(())
	}
	fn object_path(&self, id: Uuid) -> PathBuf {
		self.0.storage.join(id.simple().to_string())
	}
	pub(crate) async fn put(
		&self,
		access: &mut Access,
		area: Option<Uuid>,
		kind: &str,
		bytes: &[u8],
	) -> Result<(Uuid, String)> {
		let mut object = self
			.begin_object(access, area, kind, bytes.len() as u64)
			.await?;
		object.write_block(bytes).await?;
		object.finish(access, None).await
	}
	pub(crate) async fn begin_object(
		&self,
		access: &mut Access,
		area: Option<Uuid>,
		kind: &str,
		size: u64,
	) -> Result<PendingObject> {
		tokio::fs::create_dir_all(&self.0.storage).await?;
		if tokio::fs::symlink_metadata(&self.0.storage)
			.await?
			.file_type()
			.is_symlink()
		{
			return Err(Error::Invalid(
				"object storage must not be a symlink".into(),
			));
		}
		if !access.core_gc_complete {
			self.reconcile_orphans(access).await?;
			access.core_gc_complete = true;
		}
		let size = i64::try_from(size).map_err(|_| Error::Invalid("OBJECT_LIMIT".into()))?;
		let limit = i64::try_from(self.0.retained_bytes)
			.map_err(|_| Error::Invalid("invalid quota".into()))?;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("core_quotas"))
				.columns([Alias::new("tenant")])
				.values_panic([Expr::cust("$1")])
				.on_conflict(
					OnConflict::column(Alias::new("tenant"))
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.execute(&mut **access.tx)
		.await?;
		let reserved: Option<i64> = sqlx::query_scalar(
			&Query::update()
				.table(Alias::new("core_quotas"))
				.value(
					Alias::new("used_bytes"),
					Expr::col(Alias::new("used_bytes")).add(Expr::cust("$2")),
				)
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("used_bytes")).lte(Expr::cust("$3 - $2")))
				.returning(Query::returning().column(Alias::new("used_bytes")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.bind(size)
		.bind(limit)
		.fetch_optional(&mut **access.tx)
		.await?;
		if reserved.is_none() {
			return Err(Error::Conflict(
				"STORAGE_QUOTA: retained object quota exceeded".into(),
			));
		}
		tokio::fs::create_dir_all(&self.0.storage).await?;
		if tokio::fs::symlink_metadata(&self.0.storage)
			.await?
			.file_type()
			.is_symlink()
		{
			return Err(Error::Invalid(
				"object storage must not be a symlink".into(),
			));
		}
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			tokio::fs::set_permissions(&self.0.storage, std::fs::Permissions::from_mode(0o700))
				.await?;
		}
		let id = Uuid::new_v4();
		// Ownership survives a database rollback or process crash. The advisory
		// lock keeps a collector from mistaking an uncommitted object for an orphan.
		sqlx::query(
			&Query::select()
				.expr(Expr::cust("pg_advisory_xact_lock(hashtextextended($1, 0))"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(format!("core-object:{id}"))
		.execute(&mut **access.tx)
		.await?;
		let intents = self.0.storage.join(".intents");
		tokio::fs::create_dir_all(&intents).await?;
		let mut intent_options = tokio::fs::OpenOptions::new();
		intent_options.write(true).create_new(true);
		#[cfg(unix)]
		intent_options.mode(0o600);
		let mut intent = intent_options.open(intents.join(id.to_string())).await?;
		intent.write_all(&serde_json::to_vec(&serde_json::json!({"id":id,"tenant":access.identity.tenant,"area_id":area,"kind":kind,"size":size}))?).await?;
		intent.sync_all().await?;
		tokio::fs::File::open(&intents).await?.sync_all().await?;
		let mut options = tokio::fs::OpenOptions::new();
		options.write(true).create_new(true);
		#[cfg(unix)]
		options.mode(0o600);
		let file = options.open(self.object_path(id)).await?;
		Ok(PendingObject {
			id,
			area,
			kind: kind.into(),
			size: size as u64,
			written: 0,
			file,
			hash: Sha256::new(),
			root: self.0.storage.clone(),
		})
	}

	async fn reconcile_orphans(&self, access: &mut Access) -> Result<()> {
		let intents = self.0.storage.join(".intents");
		let mut entries = match tokio::fs::read_dir(&intents).await {
			Ok(entries) => entries,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
			Err(error) => return Err(error.into()),
		};
		let known: Vec<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.fetch_all(&mut **access.tx)
		.await?;
		let known = known.into_iter().collect::<std::collections::HashSet<_>>();
		while let Some(entry) = entries.next_entry().await? {
			let Some(id) = entry
				.file_name()
				.to_str()
				.and_then(|name| Uuid::parse_str(name).ok())
			else {
				continue;
			};
			if known.contains(&id) {
				continue;
			}
			if !entry.file_type().await?.is_file() {
				return Err(Error::Conflict("unsafe object ownership intent".into()));
			}
			let metadata: serde_json::Value =
				serde_json::from_slice(&tokio::fs::read(entry.path()).await?)?;
			if metadata["tenant"] != access.identity.tenant {
				continue;
			}
			let acquired: bool = sqlx::query_scalar(
				&Query::select()
					.expr(Expr::cust(
						"pg_try_advisory_xact_lock(hashtextextended($1, 0))",
					))
					.to_string(PostgresQueryBuilder),
			)
			.bind(format!("core-object:{id}"))
			.fetch_one(&mut **access.tx)
			.await?;
			if !acquired {
				continue;
			}
			let exists: Option<Uuid> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_objects"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.to_string(PostgresQueryBuilder),
			)
			.bind(id)
			.fetch_optional(&mut **access.tx)
			.await?;
			if exists.is_some() {
				continue;
			}
			match tokio::fs::remove_file(self.object_path(id)).await {
				Ok(()) => {}
				Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
				Err(error) => return Err(error.into()),
			}
			tokio::fs::remove_file(entry.path()).await?;
		}
		Ok(())
	}
	async fn open_object(&self, access: &mut Access, entry: &FileEntry) -> Result<tokio::fs::File> {
		let metadata: Option<(String, i64)> = sqlx::query_as(
			&Query::select()
				.columns([Alias::new("digest"), Alias::new("size")])
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(entry.file_id)
		.bind(&access.identity.tenant)
		.fetch_optional(&mut **access.tx)
		.await?;
		if metadata != Some((entry.digest.clone(), entry.size as i64)) {
			return Err(Error::NotFound("file unavailable".into()));
		}
		let mut options = tokio::fs::OpenOptions::new();
		options.read(true);
		#[cfg(unix)]
		options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
		let file = options.open(self.object_path(entry.file_id)).await?;
		let metadata = file.metadata().await?;
		#[cfg(unix)]
		{
			use std::os::unix::fs::MetadataExt;
			if metadata.nlink() != 1 {
				return Err(Error::Conflict("unsafe object alias".into()));
			}
		}
		if !metadata.is_file() || metadata.len() != entry.size {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		Ok(file)
	}
	pub(crate) async fn read(&self, access: &mut Access, entry: &FileEntry) -> Result<Vec<u8>> {
		let file = self.open_object(access, entry).await?;
		let mut bytes = Vec::new();
		file.take(entry.size + 1).read_to_end(&mut bytes).await?;
		if bytes.len() as u64 != entry.size || digest(&bytes) != entry.digest {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		Ok(bytes)
	}
	pub(crate) async fn text_file(
		&self,
		access: &mut Access,
		area: Uuid,
		path: String,
		text: &str,
		scope: FileScope,
		provenance: serde_json::Value,
	) -> Result<FileEntry> {
		validate_path(&path)?;
		let (id, digest) = self
			.put(access, Some(area), "working", text.as_bytes())
			.await?;
		Ok(FileEntry {
			file_id: id,
			path,
			digest,
			size: text.len() as u64,
			media_type: "text/plain; charset=utf-8".into(),
			scope,
			provenance,
		})
	}

	/// Transfer chunks are provisional until the receiving side verifies the
	/// complete size and digest; they must never be disclosed as verified text.
	pub(crate) async fn read_chunk(
		&self,
		access: &mut Access,
		entry: &FileEntry,
		offset: u64,
	) -> Result<Vec<u8>> {
		if offset > entry.size {
			return Err(Error::Invalid("INVALID_READ_RANGE".into()));
		}
		let mut file = self.open_object(access, entry).await?;
		file.seek(std::io::SeekFrom::Start(offset)).await?;
		let mut bytes = Vec::new();
		file.take((entry.size - offset).min(4 << 20))
			.read_to_end(&mut bytes)
			.await?;
		Ok(bytes)
	}
}

impl Runtime {
	pub(crate) async fn verified(
		&self,
		access: &mut Access,
		entry: &FileEntry,
	) -> Result<tokio::fs::File> {
		let mut file = self.open_object(access, entry).await?;
		let mut hash = Sha256::new();
		let mut total = 0;
		let mut buffer = vec![0; 65536];
		loop {
			let n = file.read(&mut buffer).await?;
			if n == 0 {
				break;
			}
			total += n as u64;
			hash.update(&buffer[..n]);
		}
		if total != entry.size || format!("{:x}", hash.finalize()) != entry.digest {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		file.rewind().await?;
		Ok(file)
	}
	pub(crate) async fn copy_owned(
		&self,
		access: &mut Access,
		area: Uuid,
		kind: &str,
		entry: &FileEntry,
	) -> Result<FileEntry> {
		self.copy_object(access, Some(area), kind, entry).await
	}
	pub(crate) async fn copy_object(
		&self,
		access: &mut Access,
		area: Option<Uuid>,
		kind: &str,
		entry: &FileEntry,
	) -> Result<FileEntry> {
		let mut source = self.open_object(access, entry).await?;
		let mut target = self.begin_object(access, area, kind, entry.size).await?;
		let mut buffer = vec![0; 65536];
		loop {
			let n = source.read(&mut buffer).await?;
			if n == 0 {
				break;
			}
			target.write_block(&buffer[..n]).await?;
		}
		let (file_id, digest) = target.finish(access, Some(&entry.digest)).await?;
		Ok(FileEntry {
			file_id,
			digest,
			..entry.clone()
		})
	}
	pub(crate) async fn reserve(&self, access: &mut Access, bytes: i64) -> Result<()> {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("core_quotas"))
				.columns([Alias::new("tenant")])
				.values_panic([Expr::cust("$1")])
				.on_conflict(
					OnConflict::column(Alias::new("tenant"))
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.execute(&mut **access.tx)
		.await?;
		let changed = sqlx::query(
			&Query::update()
				.table(Alias::new("core_quotas"))
				.value(
					Alias::new("used_bytes"),
					Expr::col(Alias::new("used_bytes")).add(Expr::cust("$2")),
				)
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.and_where(Expr::cust("used_bytes + $2 BETWEEN 0 AND $3"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.bind(bytes)
		.bind(self.0.retained_bytes as i64)
		.execute(&mut **access.tx)
		.await?
		.rows_affected();
		if changed != 1 {
			return Err(Error::Conflict("STORAGE_QUOTA".into()));
		}
		Ok(())
	}
	/// The caller first commits a tombstone that blocks new readers/writers.
	/// Unlink is idempotent: a crash between unlink and the database commit is
	/// retried against the same UUID; quota is released only after unlink.
	/// Internal reconciliation only: the caller must lock and validate a
	/// committed cleanup/expiry record or publication tombstone for this object.
	pub(crate) async fn erase_committed(
		&self,
		tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
		tenant: &str,
		id: Uuid,
	) -> Result<()> {
		let size: Option<i64> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("size"))
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(tenant)
		.fetch_optional(&mut **tx)
		.await?;
		let Some(size) = size else { return Ok(()) };
		match tokio::fs::remove_file(self.object_path(id)).await {
			Ok(()) => {}
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
			Err(e) => return Err(e.into()),
		}
		tokio::fs::File::open(&self.0.storage)
			.await?
			.sync_all()
			.await?;
		sqlx::query(
			&Query::delete()
				.from_table(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.execute(&mut **tx)
		.await?;
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_quotas"))
				.value(
					Alias::new("used_bytes"),
					Expr::col(Alias::new("used_bytes")).sub(Expr::cust("$2")),
				)
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(tenant)
		.bind(size)
		.execute(&mut **tx)
		.await?;
		Ok(())
	}
}
