//! Storage and live grant checks for remote Thread Attachment delivery.
use super::capability_records::{domain, native};
use crate::{
	Error as NativeError,
	apps::execution::capabilities::{
		serializers::{contracts::FileEntry, records::Record as NativeRecord},
		services::records,
	},
	authorization::access::Access,
	federation::Federation,
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::transfer::receiver::{ReceiverRepository, ReceiverScope},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::{
	capabilities::{
		operations::MountedFile,
		records::Record,
		transfer::{Description, receiver::ManifestLimits},
	},
	federation::delivery::{MESSAGE_BYTES, MESSAGE_FILES},
	nodes::NodeName,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) struct Repository<'a> {
	pub(crate) federation: &'a Federation,
	pub(crate) node: NodeName,
	pub(crate) address: String,
	pub(crate) grant: Uuid,
	pub(crate) description: Description,
	pub(crate) completion: bool,
}

struct Scope<'a> {
	store: &'a Store,
	access: Access,
	node: &'a NodeName,
}

// Keep the storage key stable for messages already staged before this upgrade.
fn incoming_record_id(id: Uuid, node: &NodeName) -> Uuid {
	let hash = Sha256::new()
		.chain_update(b"aidash-file-transfer-in-v1\0")
		.chain_update(node.as_str())
		.chain_update(id.as_bytes())
		.finalize();
	let mut bytes = [0; 16];
	bytes.copy_from_slice(&hash[..16]);
	uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

fn logical_record(mut row: NativeRecord, id: Uuid) -> Record {
	row.id = id;
	domain(row)
}

pub(crate) async fn incoming(
	federation: &Federation,
	node: &NodeName,
	id: Uuid,
) -> crate::Result<Option<Description>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::FederationInbound);
	let row: Option<Value> = crate::database::native::query_scalar(
		&Query::select()
			.column("data")
			.from("core_records")
			.and_where(Expr::col("id").eq(Expr::value(incoming_record_id(id, node))))
			.and_where(Expr::col("kind").eq("transfer_in"))
			.and_where(Expr::col("area_id").is_null())
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(federation.store.pool.in_scope(&scope))
	.await?;
	row.map(|data| serde_json::from_value(data["description"].clone()).map_err(Into::into))
		.transpose()
}

#[async_trait]
impl ReceiverRepository for Repository<'_> {
	fn admission_enabled(&self) -> bool {
		self.federation.store.capabilities.0.admission
	}

	fn limits(&self) -> ManifestLimits {
		if self.completion {
			use aidash_domain::deliverable::{
				MAX_COMPLETION_ATTACHMENT_BYTES, MAX_COMPLETION_ATTACHMENTS,
			};
			return ManifestLimits {
				files: MAX_COMPLETION_ATTACHMENTS,
				file_bytes: MAX_COMPLETION_ATTACHMENT_BYTES,
				bytes: MAX_COMPLETION_ATTACHMENT_BYTES,
			};
		}
		let limits = &self.federation.store.capabilities.0.limits;
		ManifestLimits {
			files: MESSAGE_FILES.min(limits.share_files),
			file_bytes: MESSAGE_BYTES.min(limits.share_file_bytes),
			bytes: MESSAGE_BYTES.min(limits.share_bytes),
		}
	}

	fn node_id(&self) -> &str {
		&self.address
	}

	fn description(&self) -> &Description {
		&self.description
	}

	async fn begin(&self, source: &str) -> Result<Box<dyn ReceiverScope + '_>> {
		let (mut access, _) = crate::apps::identity::services::remote::delivery_lease(
			self.federation,
			source,
			self.grant,
		)
		.await?;
		access.set_node(self.node.clone());
		Ok(Box::new(Scope {
			store: &self.federation.store,
			access,
			node: &self.node,
		}))
	}
}

#[async_trait]
impl ReceiverScope for Scope<'_> {
	fn credential(&self) -> Uuid {
		self.access.identity.credential_id
	}

	async fn record(&mut self, id: Uuid) -> Result<Record> {
		// Serialize the first lookup even when no staging row exists yet.
		let storage_id = incoming_record_id(id, self.node);
		crate::database::native::query(
			&Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(pg_advisory_xact_lock(hashtextextended(?, 0)))".into(),
					vec![Expr::value(storage_id.to_string()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **self.access.tx)
		.await?;
		records::get(&mut self.access, storage_id, "transfer_in")
			.await
			.map(|row| logical_record(row, id))
			.map_err(Into::into)
	}

	async fn insert(&mut self, id: Uuid, data: Value, expires: DateTime<Utc>) -> Result<Record> {
		records::insert(
			&mut self.access,
			incoming_record_id(id, self.node),
			None,
			"transfer_in",
			"staging",
			data,
			Some(expires),
		)
		.await
		.map(|row| logical_record(row, id))
		.map_err(Into::into)
	}

	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let id = record.id;
		let mut row = native(record);
		row.id = incoming_record_id(id, self.node);
		records::update(&mut self.access, &mut row).await?;
		*record = logical_record(row, id);
		Ok(())
	}

	async fn reserve(&mut self, bytes: i64) -> Result<()> {
		self.store
			.capabilities
			.reserve(&mut self.access, bytes)
			.await
			.map_err(Into::into)
	}

	async fn lock_conversion(&mut self) -> Result<()> {
		let key = format!("core-transfer-conversion:{}", self.access.identity.tenant);
		crate::database::native::query(
			&Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(pg_advisory_xact_lock(hashtextextended(?, 0)))".into(),
					vec![Expr::value(key).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **self.access.tx)
		.await?;
		Ok(())
	}

	async fn put_staging(&mut self, bytes: &[u8]) -> Result<(Uuid, String)> {
		let mut pending = self
			.store
			.capabilities
			.begin_object_converting(
				&mut self.access,
				None,
				"transfer_staging",
				bytes.len() as u64,
				bytes.len() as i64,
			)
			.await?;
		pending.write_block(bytes).await?;
		pending
			.finish(&mut self.access, None)
			.await
			.map_err(Into::into)
	}

	async fn read_file(&mut self, file: &MountedFile) -> Result<Vec<u8>> {
		self.store
			.capabilities
			.read(&mut self.access, &FileEntry::from(file.clone()))
			.await
			.map_err(Into::into)
	}

	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		self.access
			.finish(result.map_err(NativeError::from))
			.await
			.map_err(Into::into)
	}
}
