//! Provisional downloads hold no database locks. Only the exact stopped receipt
//! can adopt them, through the ordinary retained-object quota and fsync owner.
//! One anonymous spool bounds file descriptors independently of file count;
//! offsets and lengths are checked before any retained object is created.
use super::Operation;
use crate::{Error, Result, store::Store};
use aidash_application::{
	ports::capabilities::processing::{OutputTransfer, ReceiptOutputScope},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::capabilities::operations::{processing::Receipt, reconciliation::Limits};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

struct Staged {
	id: Uuid,
	kind: String,
	size: u64,
	written: u64,
	digest: Sha256,
	offset: u64,
}
struct Scope<'a> {
	store: &'a Store,
	operation: Operation,
	spool: tokio::fs::File,
	pending: Option<Staged>,
	objects: Vec<(Uuid, String, u64, String, u64)>,
	bytes: u64,
}
fn bound(operation: &Operation, receipt: &Receipt) -> bool {
	operation.id == receipt.id
		&& operation.digest == receipt.digest
		&& operation.runner_instance == receipt.runner_instance
		&& operation.runner_instance.is_some()
		&& operation.state == "uncertain"
		&& operation.result["termination_confirmed"] == true
}
pub(super) async fn begin<'a>(
	store: &'a Store,
	receipt: &Receipt,
) -> Result<Box<dyn ReceiptOutputScope + 'a>> {
	let operation: Operation = crate::database::native::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("core_operations"))
			.and_where(Expr::col("id").eq(Expr::value(receipt.id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(store.pool.in_scope(&TenancyScope::Deployment(
		DeploymentScopeReason::CapabilityMaintenance,
	)))
	.await?;
	if !bound(&operation, receipt) {
		return Err(Error::Forbidden);
	}
	Ok(Box::new(Scope {
		store,
		operation,
		spool: tokio::fs::File::from_std(tempfile::tempfile()?),
		pending: None,
		objects: vec![],
		bytes: 0,
	}))
}
#[async_trait]
impl OutputTransfer for Scope<'_> {
	async fn request(
		&mut self,
		method: &str,
		path: &str,
		body: Option<Value>,
	) -> aidash_application::Result<Value> {
		super::reconciliation::Repository { store: self.store }
			.runner_request(method, path, body)
			.await
	}
	async fn begin_output(&mut self, size: u64) -> aidash_application::Result<()> {
		self.bytes = self.bytes.checked_add(size).ok_or(Error::Forbidden)?;
		if self.bytes > self.store.capabilities.0.temporary_bytes || self.pending.is_some() {
			return Err(Error::Conflict("runner output staging quota".into()).into());
		}
		let id = Uuid::new_v4();
		self.pending = Some(Staged {
			id,
			kind: "working".into(),
			size,
			written: 0,
			digest: Sha256::new(),
			offset: self.bytes - size,
		});
		Ok(())
	}
	async fn write_output(&mut self, bytes: &[u8]) -> aidash_application::Result<()> {
		let pending = self.pending.as_mut().ok_or(Error::Forbidden)?;
		pending.written = pending
			.written
			.checked_add(bytes.len() as u64)
			.ok_or(Error::Forbidden)?;
		if pending.written > pending.size {
			return Err(Error::Conflict("OBJECT_SIZE_CHANGED".into()).into());
		}
		self.spool.write_all(bytes).await.map_err(Error::from)?;
		pending.digest.update(bytes);
		Ok(())
	}
	async fn finish_output(
		&mut self,
		expected: &str,
	) -> aidash_application::Result<(Uuid, String)> {
		let pending = self.pending.take().ok_or(Error::Forbidden)?;
		let digest = format!("{:x}", pending.digest.finalize());
		if pending.written != pending.size || digest != expected {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()).into());
		}
		self.spool.flush().await.map_err(Error::from)?;
		self.objects.push((
			pending.id,
			pending.kind,
			pending.size,
			digest.clone(),
			pending.offset,
		));
		Ok((pending.id, digest))
	}
}
use aidash_application::ports::capabilities::processing::OperationProcessingRepository;
#[async_trait]
impl ReceiptOutputScope for Scope<'_> {
	fn limits(&self) -> Limits {
		let profile = &self.store.capabilities.0;
		Limits {
			admission: profile.admission,
			working_bytes: profile.working_bytes,
			output_bytes: profile.output_bytes,
			read_bytes: profile.limits.read_bytes,
		}
	}
	async fn put(
		&mut self,
		kind: &str,
		bytes: &[u8],
	) -> aidash_application::Result<(Uuid, String)> {
		self.begin_output(bytes.len() as u64).await?;
		self.pending.as_mut().unwrap().kind = kind.into();
		self.write_output(bytes).await?;
		self.finish_output(&format!("{:x}", Sha256::digest(bytes)))
			.await
	}
	async fn commit(
		mut self: Box<Self>,
		proof: &str,
		output: Value,
	) -> aidash_application::Result<()> {
		let scope = TenancyScope::tenant(&self.operation.tenant)?;
		let mut tx = self.store.pool.begin(&scope).await?;
		crate::transactions::gate::read_in(&mut tx).await?;
		let area: Option<Uuid> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_areas"))
				.and_where(Expr::col("id").eq(Expr::value(self.operation.area_id)))
				.and_where(Expr::col("tenant").eq(Expr::value(self.operation.tenant.clone())))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		if area.is_none() {
			return Err(Error::Forbidden.into());
		}
		let current: Operation = crate::database::native::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(Alias::new("core_operations"))
				.and_where(Expr::col("id").eq(Expr::value(self.operation.id)))
				.and_where(Expr::col("tenant").eq(Expr::value(self.operation.tenant.clone())))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await?;
		if !bound(
			&current,
			&Receipt {
				id: self.operation.id,
				digest: self.operation.digest.clone(),
				runner_instance: self.operation.runner_instance.clone(),
				uncertain: true,
			},
		) || current.area_id != self.operation.area_id
			|| current.epoch != self.operation.epoch
			|| current.generation != self.operation.generation
		{
			return Err(Error::Forbidden.into());
		}
		if current.result["stopped_output_digest"] == proof {
			return Ok(()); // Already adopted: no second objects or quota charge.
		}
		if !current.result["stopped_output_digest"].is_null()
			|| output.is_null()
			|| proof.len() != 64
			|| !proof.bytes().all(|b| b.is_ascii_hexdigit())
		{
			return Err(Error::Forbidden.into());
		}
		let objects = std::mem::take(&mut self.objects);
		for (id, kind, size, digest, offset) in &objects {
			self.spool
				.seek(std::io::SeekFrom::Start(*offset))
				.await
				.map_err(Error::from)?;
			let mut target = self
				.store
				.capabilities
				.begin_object_in(
					tx.as_mut(),
					&current.tenant,
					*id,
					Some(current.area_id),
					kind,
					*size,
				)
				.await?;
			let mut buffer = vec![0; 65536];
			let mut remaining = *size;
			while remaining != 0 {
				let length = remaining.min(buffer.len() as u64) as usize;
				let read = self
					.spool
					.read(&mut buffer[..length])
					.await
					.map_err(Error::from)?;
				if read == 0 {
					return Err(Error::Conflict("OBJECT_INTEGRITY".into()).into());
				}
				remaining -= read as u64;
				target.write_block(&buffer[..read]).await?;
			}
			target
				.finish_in(tx.as_mut(), &current.tenant, Some(digest))
				.await?;
		}

		let mut result = current.result;
		result["stopped_output_digest"] = Value::String(proof.into());
		result["stopped_output"] = output;
		crate::database::native::query(
			&Query::update()
				.table(Alias::new("core_operations"))
				.value(Alias::new("result"), result)
				.and_where(Expr::col("id").eq(Expr::value(current.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(())
	}
}
