//! One spool holds stdout and displays until the stopped operation proves their origin.
use super::Operation;
use crate::{Error, Result, store::Store};
use aidash_application::{
	ports::capabilities::processing::ReceiptOutputScope,
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

struct Scope<'a> {
	store: &'a Store,
	operation: Operation,
	spool: tokio::fs::File,
	objects: Vec<(Uuid, String, u64, String, u64)>,
	bytes: u64,
}
fn bound(operation: &Operation, receipt: &Receipt) -> bool {
	operation.id == receipt.id
		&& operation.digest == receipt.digest
		&& operation.runner_instance == receipt.runner_instance
		&& operation.runner_instance.is_some()
		// A Node cancellation withdrawal keeps its outcome while adopting the
		// stopped output the Runner retained as `uncertain`.
		&& matches!(operation.state.as_str(), "uncertain" | "withdrawn")
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
		objects: vec![],
		bytes: 0,
	}))
}
#[async_trait]
impl ReceiptOutputScope for Scope<'_> {
	fn limits(&self) -> Limits {
		let profile = &self.store.capabilities.0;
		Limits {
			admission: profile.admission,
			working_bytes: profile.working_bytes,
			output_bytes: profile.output_bytes,
			read_bytes: profile.limits.read_bytes,
			maximum_seconds: profile.maximum_seconds,
		}
	}
	async fn put(
		&mut self,
		kind: &str,
		bytes: &[u8],
	) -> aidash_application::Result<(Uuid, String)> {
		let size = bytes.len() as u64;
		let offset = self.bytes;
		self.bytes = offset.checked_add(size).ok_or(Error::Forbidden)?;
		if self.bytes > self.store.capabilities.0.temporary_bytes {
			return Err(Error::Conflict("runner output staging quota".into()).into());
		}
		self.spool.write_all(bytes).await.map_err(Error::from)?;
		self.spool.flush().await.map_err(Error::from)?;
		let id = Uuid::new_v4();
		let digest = format!("{:x}", Sha256::digest(bytes));
		self.objects
			.push((id, kind.to_owned(), size, digest.clone(), offset));
		Ok((id, digest))
	}
	async fn commit(
		mut self: Box<Self>,
		proof: &str,
		output: Value,
	) -> aidash_application::Result<()> {
		let scope = TenancyScope::tenant(&self.operation.tenant)?;
		let mut tx = self.store.pool.begin(&scope).await?;
		crate::transactions::gate::read_in(&mut tx).await?;
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
					&mut tx,
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
