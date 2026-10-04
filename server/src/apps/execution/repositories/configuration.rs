//! Native version registration preserves provenance and historical attachments in one transaction.
use super::capability_records::domain;
use crate::apps::execution::capabilities::{
	serializers::configuration::Configure as NativeConfigure,
	services::{references, sessions},
};
use crate::{
	Result as NativeResult,
	authorization::access::Access,
	registry::{EntityRef, Entry},
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::configuration::{ConfigurationScope, Limits},
};
use aidash_domain::capabilities::{configuration::Configure, records::Record};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) store: &'a Store,
	pub(crate) access: &'a mut Access,
}
impl From<NativeConfigure> for Configure {
	fn from(v: NativeConfigure) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			source_version: v.source_version,
			new_version: v.new_version,
			core_capabilities: v.core_capabilities,
			skill_attachments: v.skill_attachments,
			skill_roots: v.skill_roots,
			reference_attachments: v.reference_attachments,
		}
	}
}
#[async_trait]
impl ConfigurationScope for Scope<'_> {
	fn principal(&self) -> &str {
		&self.access.identity.subject
	}
	fn limits(&self) -> Limits {
		let l = &self.store.capabilities.0.limits;
		Limits {
			files: l.reference_files,
			bytes: l.reference_bytes,
			text_bytes: l.reference_text_bytes,
		}
	}
	async fn entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		crate::authorization::catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		sessions::cached(self.access, key, digest)
			.await
			.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()> {
		sessions::cache(self.access, key, digest, result)
			.await
			.map_err(Into::into)
	}
	async fn reference(&mut self, id: Uuid) -> Result<Record> {
		references::get(self.access, id, "reference.read")
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn register(&mut self, entry: &Entry) -> Result<bool> {
		crate::registry::register_in(&mut self.access.tx, entry, &self.store.node_id)
			.await
			.map_err(Into::into)
	}
	async fn provenance(&mut self, source: &EntityRef, entry: &Entry) -> Result<()> {
		crate::marketplace::propagate_provenance(
			&mut self.access.tx,
			source,
			entry,
			&self.access.identity.tenant,
		)
		.await
		.map_err(Into::into)
	}
	async fn preserve_documents(&mut self, id: &str, version: &str, source: &str) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = id;
				let query_bind_2 = version;
				let query_bind_3 = source;
				sqlx::query(
					&Query::insert()
						.into_table(Alias::new("agent_knowledge"))
						.columns(["agent_id", "agent_version", "documents"].map(Alias::new))
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.column(Alias::new("documents"))
								.from(Alias::new("agent_knowledge"))
								.and_where(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"agent_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									)),
								)
								.and_where(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"agent_version",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_3.to_owned()).into()],
									)),
								)
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn event(&mut self, kind: &str, data: Value) -> Result<()> {
		self.store
			.event(&mut self.access.tx, None, kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
