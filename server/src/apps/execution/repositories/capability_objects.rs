//! Immutable node-managed objects. User paths never become host filesystem paths.
use crate::apps::execution::capabilities::services::core::{
	Runtime,
	contracts::{FileEntry, FileScope},
};
use crate::{Error, Result, authorization::access::Access};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use reinhardt::query::{Alias, Expr, IntoValue as _, OnConflict, PostgresQueryBuilder, Query};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

// Cleanup retries must not consume the connection pool while a publication
// with many files still owns its object locks.
static UPLOAD_CLEANUP: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

struct ObjectReservation<'a> {
	id: Uuid,
	area: Option<Uuid>,
	kind: &'a str,
	size: u64,
	grandfather_existing: bool,
}

pub(crate) struct PendingObject {
	runtime: Runtime,
	id: Uuid,
	area: Option<Uuid>,
	kind: String,
	size: u64,
	written: u64,
	file: Option<tokio::fs::File>,
	hash: Sha256,
	pool: crate::database::native::Pool,
	tenant: String,
	/// Skips the retained-capacity refusal for bytes the Tenant already held
	/// before they moved into the object store (legacy attachment backfill).
	grandfather_existing: bool,
}

impl Drop for PendingObject {
	fn drop(&mut self) {
		self.runtime
			.schedule_release(self.pool.clone(), self.tenant.clone(), self.id);
	}
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
		self.file
			.as_mut()
			.expect("pending upload file")
			.write_all(bytes)
			.await?;
		self.hash.update(bytes);
		Ok(())
	}
	pub(crate) async fn finish(
		self,
		access: &mut Access,
		expected: Option<&str>,
	) -> Result<(Uuid, String)> {
		self.finish_in(access.tx.as_mut(), &access.identity.tenant, expected)
			.await
	}
	pub(crate) async fn finish_for(
		self,
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		expected: Option<&str>,
	) -> Result<(Uuid, String)> {
		self.finish_in(tx.as_mut(), tenant, expected).await
	}
	pub(crate) async fn finish_in(
		mut self,
		tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
		tenant: &str,
		expected: Option<&str>,
	) -> Result<(Uuid, String)> {
		let hash = format!("{:x}", std::mem::take(&mut self.hash).finalize());
		if self.written != self.size || expected.is_some_and(|expected| hash != expected) {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		self.file
			.as_mut()
			.expect("pending upload file")
			.sync_all()
			.await?;
		let tenant = tenant.to_owned();
		// Persist the proposed digest before exposing a blob so a later
		// maintenance pass can find a write whose DB transaction rolled back.
		let intent_dir = self.runtime.0.storage.join(".intents");
		let intent_path = intent_dir.join(self.id.to_string());
		let temporary = intent_dir.join(format!(".{}.new", self.id));
		let mut options = tokio::fs::OpenOptions::new();
		options.write(true).create_new(true);
		#[cfg(unix)]
		options.mode(0o600);
		let mut intent = options.open(&temporary).await?;
		intent
			.write_all(&serde_json::to_vec(
				&serde_json::json!({"id": self.id, "tenant": tenant, "digest": hash}),
			)?)
			.await?;
		intent.sync_all().await?;
		tokio::fs::rename(&temporary, &intent_path).await?;
		tokio::fs::File::open(&intent_dir).await?.sync_all().await?;
		let digest: aidash_domain::capabilities::Digest = hash
			.parse()
			.map_err(|_| Error::Invalid("invalid object digest".into()))?;
		// Publish the reservation's digest before uploading, so GC must wait
		// until the caller's publication has committed or rolled back.
		{
			let mut tx = self.pool.begin(&MAINTENANCE).await?;
			let changed = crate::database::native::query(
				&Query::update()
					.table(Alias::new("core_blob_uploads"))
					.value(Alias::new("digest"), hash.clone())
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(self.id)))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(&tenant)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
			.rows_affected();
			if changed != 1 {
				return Err(Error::Conflict("UPLOAD_RESERVATION_CHANGED".into()));
			}
			if !self.grandfather_existing {
				self.runtime
					.check_upload_quota(&mut tx, &tenant, 0, true)
					.await?;
			}
			tx.commit().await?;
		}
		// The digest lock serializes writers without holding a Tenant quota row
		// while the object store receives bytes.
		let mut blob_tx = self.pool.begin(&MAINTENANCE).await?;
		let lock = format!("core-blob:{tenant}:{hash}");
		crate::database::native::query(
			&Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(pg_advisory_xact_lock(hashtextextended(?, 0)))".into(),
					vec![Expr::value(lock).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *blob_tx)
		.await?;
		let existing: Option<i64> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("size"))
				.from(Alias::new("core_blobs"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(&tenant)))
				.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(&hash)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *blob_tx)
		.await?;
		let store = self.runtime.blob_store().await?;
		if let Some(size) = existing {
			if size != self.size as i64 {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			if !store.exists(&tenant, digest).await? {
				// A previous garbage collection may have unlinked an unreferenced
				// blob before its transaction rolled back. Restore it on verified put.
				self.file
					.as_mut()
					.expect("pending upload file")
					.rewind()
					.await?;
				store
					.put_if_absent(
						&tenant,
						digest,
						self.size,
						Box::pin(self.file.take().expect("pending upload file")),
					)
					.await?;
			}
		} else {
			// An uncommitted prior writer may have left bytes without a DB
			// owner. Under the digest lock, discard them before verified put.
			store.delete(&tenant, digest).await?;
			self.file
				.as_mut()
				.expect("pending upload file")
				.rewind()
				.await?;
			store
				.put_if_absent(
					&tenant,
					digest,
					self.size,
					Box::pin(self.file.take().expect("pending upload file")),
				)
				.await?;
			self.runtime
				.adjust_quota_tx(&mut blob_tx, &tenant, self.size as i64, "used_bytes")
				.await?;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("core_blobs"))
					.columns(["tenant", "digest", "size"].map(Alias::new))
					.values(vec![
						tenant.clone().into(),
						hash.clone().into(),
						(self.size as i64).into(),
					])
					.map_err(Error::Invalid)?
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *blob_tx)
			.await?;
		}
		blob_tx.commit().await?;
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("core_objects"))
				.columns(["id", "tenant", "area_id", "kind", "digest", "size"].map(Alias::new))
				.values(vec![
					self.id.into(),
					tenant.into(),
					self.area.into_value(),
					self.kind.clone().into(),
					hash.clone().into(),
					(self.size as i64).into(),
				])
				.map_err(Error::Invalid)?
				.to_string(PostgresQueryBuilder),
		)
		.execute(tx)
		.await?;
		Ok((self.id, hash))
	}
}

/// Serialize changes to a Tenant's retained-byte accounting.
pub(crate) async fn lock_tenant_quota(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
) -> Result<()> {
	crate::database::native::query(
		&Query::insert()
			.into_table(Alias::new("core_quotas"))
			.columns([Alias::new("tenant")])
			.values(vec![tenant.into()])
			.map_err(Error::Invalid)?
			.on_conflict(
				OnConflict::column(Alias::new("tenant"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	let _: String = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("tenant"))
			.from(Alias::new("core_quotas"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
			.lock(reinhardt::query::LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **tx)
	.await?;
	Ok(())
}

pub fn digest(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}
pub fn validate_path(path: &str) -> Result<()> {
	aidash_domain::registry::rules::validate_path(path).map_err(Into::into)
}
impl Runtime {
	fn schedule_release(&self, pool: crate::database::native::Pool, tenant: String, id: Uuid) {
		let runtime = self.clone();
		if let Ok(handle) = tokio::runtime::Handle::try_current() {
			handle.spawn(async move {
				for _ in 0..40 {
					let result = {
						let Ok(_permit) = UPLOAD_CLEANUP.acquire().await else {
							return;
						};
						runtime.release_upload(&pool, &tenant, id).await
					};
					match result {
						Ok(true) => return,
						Ok(false) => {
							tokio::time::sleep(std::time::Duration::from_millis(250)).await
						}
						Err(error) => {
							tracing::warn!(%id, %error, "upload reservation cleanup pending");
							return;
						}
					}
				}
				tracing::warn!(%id, "upload reservation cleanup retries exhausted");
			});
		}
	}
	async fn reserve_upload(
		&self,
		pool: &crate::database::native::Pool,
		tenant: &str,
		id: Uuid,
		size: i64,
		grandfather_existing: bool,
	) -> Result<()> {
		let mut tx = pool.begin(&MAINTENANCE).await?;
		let lock = format!("core-upload-quota:{tenant}");
		crate::database::native::query(
			&Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(pg_advisory_xact_lock(hashtextextended(?, 0)))".into(),
					vec![Expr::value(lock).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		// Bound temporary spool usage before its digest is known. Once hashed,
		// check retained capacity with deduplication before remote upload.
		if !grandfather_existing {
			self.check_upload_quota(&mut tx, tenant, size, false)
				.await?;
		}
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("core_blob_uploads"))
				.columns(["id", "tenant", "size"].map(Alias::new))
				.values(vec![id.into(), tenant.into(), size.into()])
				.map_err(Error::Invalid)?
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(())
	}

	async fn check_upload_quota(
		&self,
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		additional: i64,
		include_used: bool,
	) -> Result<()> {
		// Serialize the capacity check with commits that charge retained bytes.
		// The lock is released with this short reservation transaction, before
		// any bytes are sent to the object store.
		self.adjust_quota_tx(tx, tenant, 0, "used_bytes").await?;
		let used: i64 = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("used_bytes"))
				.from(Alias::new("core_quotas"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?;
		let reservations: Vec<i64> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("size"))
				.from(Alias::new("core_blob_uploads"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				// Once a blob is charged it must not also consume a pending
				// reservation while the publication transaction finishes.
				.and_where(Expr::not_exists(
					Query::select()
						.expr(Expr::value(1))
						.from(Alias::new("core_blobs"))
						.and_where(Expr::col((Alias::new("core_blobs"), Alias::new("tenant")))
							.equals((Alias::new("core_blob_uploads"), Alias::new("tenant"))))
						.and_where(Expr::col((Alias::new("core_blobs"), Alias::new("digest")))
							.equals((Alias::new("core_blob_uploads"), Alias::new("digest"))))
						.to_owned(),
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&mut **tx)
		.await?;
		let total = reservations
			.into_iter()
			.try_fold(
				additional
					.checked_add(if include_used { used } else { 0 })
					.ok_or_else(|| Error::Conflict("STORAGE_QUOTA".into()))?,
				i64::checked_add,
			)
			.ok_or_else(|| Error::Conflict("STORAGE_QUOTA".into()))?;
		if total > self.0.retained_bytes as i64 {
			return Err(Error::Conflict("STORAGE_QUOTA".into()));
		}
		Ok(())
	}

	async fn release_upload(
		&self,
		pool: &crate::database::native::Pool,
		tenant: &str,
		id: Uuid,
	) -> Result<bool> {
		let mut tx = pool.begin(&MAINTENANCE).await?;
		let lock = format!("core-object:{id}");
		let acquired: bool = crate::database::native::query_scalar(
			&Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(pg_try_advisory_xact_lock(hashtextextended(?, 0)))".into(),
					vec![Expr::value(lock).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut *tx)
		.await?;
		if !acquired {
			return Ok(false);
		}
		let digest: Option<Option<String>> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("digest"))
				.from(Alias::new("core_blob_uploads"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		if let Some(Some(hash)) = digest {
			let lock = format!("core-blob:{tenant}:{hash}");
			let acquired: bool = crate::database::native::query_scalar(
				&Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(pg_try_advisory_xact_lock(hashtextextended(?, 0)))".into(),
						vec![Expr::value(lock).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut *tx)
			.await?;
			if !acquired {
				return Ok(false);
			}
			let committed: Option<String> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("digest"))
					.from(Alias::new("core_blobs"))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
					.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(&hash)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut *tx)
			.await?;
			if committed.is_none() {
				let digest = hash
					.parse()
					.map_err(|_| Error::Conflict("OBJECT_INTEGRITY".into()))?;
				self.blob_store().await?.delete(tenant, digest).await?;
			}
		}
		crate::database::native::query(
			&Query::delete()
				.from_table(Alias::new("core_blob_uploads"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		for path in [
			self.object_path(id),
			self.0.storage.join(".intents").join(id.to_string()),
			self.0.storage.join(".intents").join(format!(".{id}.new")),
		] {
			match tokio::fs::remove_file(path).await {
				Ok(()) => {}
				Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
				Err(error) => return Err(error.into()),
			}
		}
		Ok(true)
	}
	/// Remove durable local upload intents and spools for a deleted Tenant.
	pub(crate) async fn purge_tenant_intents(
		&self,
		pool: &crate::database::native::Pool,
		tenant: &str,
	) -> Result<()> {
		async fn belongs_to(
			path: &std::path::Path,
			tenant: &str,
			id: Uuid,
		) -> Result<Option<bool>> {
			let mut options = tokio::fs::OpenOptions::new();
			options.read(true);
			#[cfg(unix)]
			options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
			let file = match options.open(path).await {
				Ok(file) => file,
				Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
				Err(error) => return Err(error.into()),
			};
			let metadata = file.metadata().await?;
			if !metadata.is_file() || metadata.len() > 8192 {
				return Ok(None);
			}
			let mut bytes = Vec::new();
			file.take(8193).read_to_end(&mut bytes).await?;
			let Ok(intent) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
				return Ok(None);
			};
			if intent["tenant"].as_str().is_none() || intent["id"] != id.to_string() {
				return Ok(None);
			}
			Ok(Some(intent["tenant"] == tenant))
		}
		let mut entries = match tokio::fs::read_dir(self.0.storage.join(".intents")).await {
			Ok(entries) => entries,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
			Err(error) => return Err(error.into()),
		};
		while let Some(entry) = entries.next_entry().await? {
			let name = entry.file_name();
			let Some(name) = name.to_str() else { continue };
			let id = name
				.strip_prefix('.')
				.and_then(|s| s.strip_suffix(".new"))
				.unwrap_or(name);
			let Ok(id) = Uuid::parse_str(id) else {
				continue;
			};
			// Do not wait for unrelated Tenants' in-flight uploads.
			if belongs_to(&entry.path(), tenant, id).await? == Some(false) {
				continue;
			}
			let mut tx = pool.begin(&MAINTENANCE).await?;
			let lock = format!("core-object:{id}");
			crate::database::native::query(
				&Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(pg_advisory_xact_lock(hashtextextended(?, 0)))".into(),
						vec![Expr::value(lock).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
			// Re-read after locking because the writer may have replaced the intent.
			if belongs_to(&entry.path(), tenant, id).await? != Some(true) {
				continue;
			}
			for path in [self.object_path(id), entry.path()] {
				match tokio::fs::remove_file(path).await {
					Ok(()) => {}
					Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
					Err(error) => return Err(error.into()),
				}
			}
			tx.commit().await?;
		}
		Ok(())
	}
	/// Reconcile crash/rollback debris without impersonating a user. A writer
	/// holds the same object lock until its database transaction commits. Once
	/// acquired, absence of the object row proves these bytes have no owner.
	pub(crate) async fn reconcile_orphan_batch(
		&self,
		pool: &crate::database::native::Pool,
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
			let name = entry.file_name();
			let Some(name) = name.to_str() else {
				continue;
			};
			let temporary_name = name.strip_prefix('.').and_then(|s| s.strip_suffix(".new"));
			let temporary = temporary_name.is_some();
			let id = Uuid::parse_str(temporary_name.unwrap_or(name));
			let Ok(id) = id else {
				continue;
			};
			let mut tx = pool.begin(&MAINTENANCE).await?;
			let acquired: bool = {
				let query_bind_1 = format!("core-object:{id}");
				crate::database::native::query_scalar(
					&Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(pg_try_advisory_xact_lock(hashtextextended(?, 0)))".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut *tx)
				.await?
			};
			if !acquired {
				continue;
			}
			if temporary {
				match tokio::fs::remove_file(entry.path()).await {
					Ok(()) => {}
					Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
					Err(e) => return Err(e.into()),
				}
				tx.commit().await?;
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
			if let Some(hash) = intent["digest"].as_str() {
				let digest = hash
					.parse()
					.map_err(|_| Error::Conflict("invalid object ownership intent".into()))?;
				let tenant = intent["tenant"].as_str().ok_or(Error::Forbidden)?;
				let lock = format!("core-blob:{tenant}:{hash}");
				let acquired: bool = crate::database::native::query_scalar(
					&Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(pg_try_advisory_xact_lock(hashtextextended(?, 0)))".into(),
							vec![Expr::value(lock).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut *tx)
				.await?;
				if !acquired {
					continue;
				}
				let exists: Option<String> = crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("digest"))
						.from(Alias::new("core_blobs"))
						.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
						.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(hash)))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut *tx)
				.await?;
				if exists.is_none() {
					self.blob_store().await?.delete(tenant, digest).await?;
				}
			}
			crate::database::native::query(
				&Query::delete()
					.from_table(Alias::new("core_blob_uploads"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
			tx.commit().await?;
			match tokio::fs::remove_file(self.object_path(id)).await {
				Ok(()) => {}
				Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
				Err(e) => return Err(e.into()),
			}

			match tokio::fs::remove_file(entry.path()).await {
				Ok(()) => {}
				Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
				Err(e) => return Err(e.into()),
			}
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
		let tenant = access.identity.tenant.clone();
		self.begin_object_in(&mut access.tx, &tenant, Uuid::new_v4(), area, kind, size)
			.await
	}
	pub(crate) async fn begin_object_for(
		&self,
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		area: Option<Uuid>,
		kind: &str,
		size: u64,
		grandfather_existing: bool,
	) -> Result<PendingObject> {
		self.begin_object_with_quota(
			tx,
			tenant,
			ObjectReservation {
				id: Uuid::new_v4(),
				area,
				kind,
				size,
				grandfather_existing,
			},
		)
		.await
	}
	/// Exact retained-operation receipt owner only; this does not admit execution.
	pub(crate) async fn begin_object_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		id: Uuid,
		area: Option<Uuid>,
		kind: &str,
		size: u64,
	) -> Result<PendingObject> {
		self.begin_object_with_quota(
			tx,
			tenant,
			ObjectReservation {
				id,
				area,
				kind,
				size,
				grandfather_existing: false,
			},
		)
		.await
	}
	async fn begin_object_with_quota(
		&self,
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		reservation: ObjectReservation<'_>,
	) -> Result<PendingObject> {
		let ObjectReservation {
			id,
			area,
			kind,
			size,
			grandfather_existing,
		} = reservation;
		let size = i64::try_from(size).map_err(|_| Error::Invalid("OBJECT_LIMIT".into()))?;
		let pool = tx.pool().clone();
		let tenant = tenant.to_owned();
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
		// Ownership survives a database rollback or process crash. The advisory
		// lock keeps a collector from mistaking an uncommitted object for an orphan.
		{
			let query_bind_1 = format!("core-object:{id}");
			crate::database::native::query(
				&Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(pg_advisory_xact_lock(hashtextextended(?, 0)))".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		};
		let intents = self.0.storage.join(".intents");
		tokio::fs::create_dir_all(&intents).await?;
		let mut intent_options = tokio::fs::OpenOptions::new();
		intent_options.write(true).create_new(true);
		#[cfg(unix)]
		intent_options.mode(0o600);
		let mut intent = intent_options.open(intents.join(id.to_string())).await?;
		intent
			.write_all(&serde_json::to_vec(
				&serde_json::json!({"id":id,"tenant":tenant,"area_id":area,"kind":kind,"size":size}),
			)?)
			.await?;
		intent.sync_all().await?;
		tokio::fs::File::open(&intents).await?.sync_all().await?;
		self.reserve_upload(&pool, &tenant, id, size, grandfather_existing)
			.await?;
		let mut options = tokio::fs::OpenOptions::new();
		options.read(true).write(true).create_new(true);
		#[cfg(unix)]
		options.mode(0o600);
		let file = match options.open(self.object_path(id)).await {
			Ok(file) => file,
			Err(error) => {
				self.schedule_release(pool.clone(), tenant.clone(), id);
				return Err(error.into());
			}
		};
		Ok(PendingObject {
			runtime: self.clone(),
			id,
			area,
			kind: kind.into(),
			size: size as u64,
			written: 0,
			file: Some(file),
			hash: Sha256::new(),
			pool,
			tenant,
			grandfather_existing,
		})
	}

	async fn reconcile_orphans(&self, access: &mut Access) -> Result<()> {
		let intents = self.0.storage.join(".intents");
		let mut entries = match tokio::fs::read_dir(&intents).await {
			Ok(entries) => entries,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
			Err(error) => return Err(error.into()),
		};
		let known: Vec<Uuid> = {
			let query_bind_1 = &access.identity.tenant;
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_objects"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_all(&mut **access.tx)
			.await?
		};
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
			// Digest-bearing intents also name a possibly orphaned cloud blob;
			// only the maintenance pass removes them under the digest lock.
			if metadata["digest"].is_string() {
				continue;
			}
			let acquired: bool = {
				let query_bind_1 = format!("core-object:{id}");
				crate::database::native::query_scalar(
					&Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(pg_try_advisory_xact_lock(hashtextextended(?, 0)))".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **access.tx)
				.await?
			};
			if !acquired {
				continue;
			}
			let exists: Option<Uuid> = {
				let query_bind_1 = id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("core_objects"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **access.tx)
				.await?
			};
			if exists.is_some() {
				continue;
			}
			let reservation: Option<Uuid> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_blob_uploads"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **access.tx)
			.await?;
			if reservation.is_some() {
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
	async fn validate_object(&self, access: &mut Access, entry: &FileEntry) -> Result<()> {
		let metadata: Option<(String, i64)> = {
			let query_bind_1 = entry.file_id;
			let query_bind_2 = &access.identity.tenant;
			crate::database::native::query_as(
				&Query::select()
					.columns([Alias::new("digest"), Alias::new("size")])
					.from(Alias::new("core_objects"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["digest", "size"])
			.fetch_optional(&mut **access.tx)
			.await?
		};
		if metadata != Some((entry.digest.clone(), entry.size as i64)) {
			return Err(Error::NotFound("file unavailable".into()));
		}
		Ok(())
	}
	async fn open_object(
		&self,
		access: &mut Access,
		entry: &FileEntry,
	) -> Result<aidash_application::ports::blob_store::BlobReader> {
		self.validate_object(access, entry).await?;
		let digest = entry
			.digest
			.parse()
			.map_err(|_| Error::Conflict("OBJECT_INTEGRITY".into()))?;
		self.blob_store()
			.await?
			.get(&access.identity.tenant, digest, None)
			.await?
			.ok_or_else(|| Error::NotFound("file unavailable".into()))
	}
	pub(crate) async fn read(&self, access: &mut Access, entry: &FileEntry) -> Result<Vec<u8>> {
		let file = self.open_object(access, entry).await?;
		let mut bytes = Vec::new();
		file.take(entry.size + 1).read_to_end(&mut bytes).await?;
		if bytes.len() as u64 != entry.size {
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
		// Validate the logical reference before opening a range from the store.
		self.validate_object(access, entry).await?;
		if offset == entry.size {
			return Ok(Vec::new());
		}
		let digest = entry
			.digest
			.parse()
			.map_err(|_| Error::Conflict("OBJECT_INTEGRITY".into()))?;
		let end = offset.saturating_add(4 << 20).min(entry.size);
		let mut reader = self
			.blob_store()
			.await?
			.get(&access.identity.tenant, digest, Some(offset..end))
			.await?
			.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
		let mut bytes = Vec::new();
		reader.read_to_end(&mut bytes).await?;
		if bytes.len() as u64 != end - offset {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		Ok(bytes)
	}
}

impl Runtime {
	pub(crate) async fn verified(
		&self,
		access: &mut Access,
		entry: &FileEntry,
	) -> Result<aidash_application::ports::blob_store::BlobReader> {
		self.open_object(access, entry).await
	}

	/// Reuse a verified Tenant blob without reading or transferring its bytes.
	pub(crate) async fn reference_blob(
		&self,
		access: &mut Access,
		area: Uuid,
		kind: &str,
		hash: &str,
		size: u64,
	) -> Result<Option<Uuid>> {
		let digest = hash
			.parse()
			.map_err(|_| Error::Invalid("invalid object digest".into()))?;
		let size = i64::try_from(size).map_err(|_| Error::Invalid("OBJECT_LIMIT".into()))?;
		let tenant = access.identity.tenant.clone();
		let existing: Option<i64> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("size"))
				.from(Alias::new("core_blobs"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(&tenant)))
				.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(hash)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **access.tx)
		.await?;
		let Some(existing) = existing else {
			return Ok(None);
		};
		if existing != size {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		if !self.blob_store().await?.exists(&tenant, digest).await? {
			return Ok(None);
		}
		let id = Uuid::new_v4();
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("core_objects"))
				.columns(["id", "tenant", "area_id", "kind", "digest", "size"].map(Alias::new))
				.values(vec![
					id.into(),
					tenant.into(),
					area.into(),
					kind.into(),
					hash.into(),
					size.into(),
				])
				.map_err(Error::Invalid)?
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?;
		Ok(Some(id))
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
		// Each logical object has its own authority and lifecycle while its
		// immutable bytes are shared within the Tenant.
		let tenant = access.identity.tenant.clone();
		self.validate_object(access, entry).await?;
		let digest = entry
			.digest
			.parse()
			.map_err(|_| Error::Conflict("OBJECT_INTEGRITY".into()))?;
		if !self.blob_store().await?.exists(&tenant, digest).await? {
			return Err(Error::NotFound("file unavailable".into()));
		}
		let file_id = Uuid::new_v4();
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("core_objects"))
				.columns(["id", "tenant", "area_id", "kind", "digest", "size"].map(Alias::new))
				.values(vec![
					file_id.into(),
					tenant.into(),
					area.into_value(),
					kind.into(),
					entry.digest.clone().into(),
					(entry.size as i64).into(),
				])
				.map_err(Error::Invalid)?
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?;
		Ok(FileEntry {
			file_id,
			..entry.clone()
		})
	}
	pub(crate) async fn reserve(&self, access: &mut Access, bytes: i64) -> Result<()> {
		let tenant = access.identity.tenant.clone();
		self.adjust_quota_tx(access.tx.native_mut(), &tenant, bytes, "reserved_bytes")
			.await
	}
	async fn adjust_quota_tx(
		&self,
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		bytes: i64,
		column: &str,
	) -> Result<()> {
		{
			let query_bind_1 = tenant;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("core_quotas"))
					.columns([Alias::new("tenant")])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::column(Alias::new("tenant"))
							.do_nothing()
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};
		let changed = {
			let query_bind_1 = tenant;
			let query_bind_2 = bytes;
			let query_bind_3 = self.0.retained_bytes as i64;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("core_quotas"))
					.value_expr(
						Alias::new(column),
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(column))).add(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.and_where(
						Expr::col(Alias::new(column))
							.add(Expr::value(query_bind_2))
							.between(0, query_bind_3),
					)
					// A new transfer reservation must fit alongside retained bytes.
					.and_where(if column == "reserved_bytes" && bytes > 0 {
						Expr::col(Alias::new("used_bytes"))
							.add(Expr::col(Alias::new("reserved_bytes")))
							.add(Expr::value(bytes))
							.lte(query_bind_3)
					} else {
						Expr::value(true).eq(true)
					})
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		}
		.rows_affected();
		if changed != 1 {
			return Err(Error::Conflict("STORAGE_QUOTA".into()));
		}
		Ok(())
	}
	/// Remove logical ownership; a later sweep reclaims unreferenced bytes.
	/// The caller must lock and validate the relevant cleanup or expiry record.
	pub(crate) async fn erase_committed(
		&self,
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		id: Uuid,
		purging: bool,
	) -> Result<()> {
		let digest: Option<String> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("digest"))
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **tx)
		.await?;
		let Some(digest) = digest else { return Ok(()) };
		let lock = format!("core-blob:{tenant}:{digest}");
		crate::database::native::query(
			&Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(pg_advisory_xact_lock(hashtextextended(?, 0)))".into(),
					vec![Expr::value(lock).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;
		let object: Option<(i64, String)> = crate::database::native::query_as(
			&Query::select()
				.columns([Alias::new("size"), Alias::new("kind")])
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(&digest)))
				.lock(reinhardt::query::LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["size", "kind"])
		.fetch_optional(&mut **tx)
		.await?;
		let Some((_size, kind)) = object else {
			return Ok(());
		};
		if !purging
			&& matches!(
				kind.as_str(),
				"superseded_working" | "received" | "transfer_snapshot"
			) {
			let safe: Option<Uuid> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_objects"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
					.and_where(super::reclamation::no_active_hold())
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **tx)
			.await?;
			if safe.is_none() {
				return Ok(());
			}
		}
		// A revision reference uses a restrictive foreign key: it cannot be
		// reclaimed even if a stale sweep selected this object earlier.
		crate::database::native::query(
			&Query::delete()
				.from_table(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;
		// Physical deletion runs in a later transaction after this removal has
		// committed, so rolling back cannot resurrect a reference to lost bytes.
		Ok(())
	}
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use reinhardt::query::SimpleExpr;

/// The capability worker reaches every Tenant's operations and work areas.
const MAINTENANCE: TenancyScope =
	TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);
