//! Native summaries retain the caller's authority transaction and latest-record ordering.
use crate::{
	Error as NativeError, Result as NativeResult, authorization::access::Access, store::Store,
};
use aidash_application::{
	Result,
	ports::{
		generation::visibility::GenerationVisibility,
		semantic::remote_status::{StatusRepository, StatusScope},
	},
};
use aidash_domain::{
	Task,
	generation::requests::Request,
	policy::Resource,
	semantic::remote::{journal::Record as State, status::Allowance},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug)]
struct AllowanceRow {
	pub request_id: Uuid,
	pub token_limit: i64,
	pub used_tokens: i64,
	pub embedding_call_limit: i64,
	pub embedding_calls: i64,
	pub compaction_call_limit: i64,
	pub compaction_calls: i64,
}
crate::native_record!(AllowanceRow {
	request_id,
	token_limit,
	used_tokens,
	embedding_call_limit,
	embedding_calls,
	compaction_call_limit,
	compaction_calls
});

impl From<AllowanceRow> for Allowance {
	fn from(row: AllowanceRow) -> Self {
		Self {
			request_id: row.request_id,
			token_limit: row.token_limit,
			used_tokens: row.used_tokens,
			embedding_call_limit: row.embedding_call_limit,
			embedding_calls: row.embedding_calls,
			compaction_call_limit: row.compaction_call_limit,
			compaction_calls: row.compaction_calls,
		}
	}
}

pub(crate) struct Scope<'a> {
	pub store: Option<&'a Store>,
	pub access: &'a mut Access,
	pub node_id: &'a str,
}
#[async_trait]
impl GenerationVisibility for Scope<'_> {
	fn inherited_lease(&self) -> bool {
		self.access.inherited_lease
	}
	fn context(&mut self, value: Value) {
		self.access.context = value;
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		crate::bootstrap::generation_visibility_scope(self.access)
			.workspace(id)
			.await
	}
	async fn task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>> {
		crate::bootstrap::generation_visibility_scope(self.access)
			.task(id, workspace)
			.await
	}
	async fn task_visible(&mut self, task: &Task) -> Result<bool> {
		crate::bootstrap::generation_visibility_scope(self.access)
			.task_visible(task)
			.await
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		crate::bootstrap::generation_visibility_scope(self.access)
			.decide(resource, action)
			.await
	}
}
#[async_trait]
impl StatusScope for Scope<'_> {
	fn node_id(&self) -> &str {
		self.node_id
	}
	fn tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	async fn request(&mut self, id: Uuid, tenant: &str) -> Result<Option<Request>> {
		let result: NativeResult<Option<Request>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = tenant;
				crate::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("generation_requests"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND tenant=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn allowance(&mut self, id: Uuid) -> Result<Allowance> {
		let result: NativeResult<AllowanceRow> = async {
			Ok({
				let query_bind_1 = id;
				crate::database::native::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("generation_budgets"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(request_id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map(Into::into).map_err(Into::into)
	}
	async fn run_visible(&mut self, id: Uuid) -> Result<bool> {
		let result: NativeResult<bool> = async {
			let store = self.store.ok_or_else(|| {
				NativeError::Invalid("semantic status storage unavailable".into())
			})?;
			let run = store.run(id).await?;
			self.access.run_visible(&run).await
		}
		.await;
		result.map_err(Into::into)
	}
	async fn receipt(&mut self, id: Uuid) -> Result<Option<Value>> {
		let result: NativeResult<Option<Value>> = async {
			if !super::receiver_caches::readable(&mut self.access.tx, id).await? {
				return Err(NativeError::RemoteSemantic(
					aidash_domain::semantic::Failure::Invalidated,
				));
			}
			Ok({
				let query_bind_1 = id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("receipt"))
						.from(Alias::new("semantic_remote_receipts"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(run_id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by(Alias::new("created_at"), Order::Desc)
						.order_by(Alias::new("operation_id"), Order::Desc)
						.limit(1)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
}
pub(crate) struct Repository<'a> {
	pub store: &'a Store,
}
#[async_trait]
impl StatusRepository for Repository<'_> {
	async fn latest(&self, grant: Uuid) -> Result<Option<State>> {
		use super::remote_journal::Record;
		let result: NativeResult<Option<Record>> = async {
			Ok({
				let query_bind_1 = grant;
				crate::database::native::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("semantic_remote_operations"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(grant_id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by(Alias::new("created_at"), Order::Desc)
						.order_by(Alias::new("id"), Order::Desc)
						.limit(1)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&self.store.pool)
				.await?
			})
		}
		.await;
		result.map(|row| row.map(Into::into)).map_err(Into::into)
	}
}
