//! Native query snapshots and owned Generation read authority.
use crate::{
	Error,
	authorization::{access::Access, identity::Actor},
	store::Store,
};
use aidash_application::ports::generation::{
	reads::{GenerationPages, GenerationReadScope, GenerationReads},
	visibility::GenerationVisibility,
};
use aidash_domain::{
	entities::Task,
	generation::{
		policy::Spec,
		requests::{History, Request, Usage},
	},
	identity::Principal,
	policy::Resource,
};
use async_trait::async_trait;
use reinhardt::db::orm::connection::{DatabaseConnection, DatabaseConnectionLease};
use reinhardt::query::{
	Alias, ColumnRef, Expr, JoinType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr, TableRef,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct NativeReads {
	pub store: Store,
	pub actor: Actor,
	pub principal: Principal,
}
struct Pages {
	_lease: DatabaseConnectionLease,
	connection: DatabaseConnection,
}
#[async_trait]
impl GenerationPages for Pages {
	async fn page(
		&mut self,
		tenant: &str,
		offset: usize,
	) -> aidash_application::Result<Vec<Request>> {
		crate::apps::execution::generation::models::GenerationRequest::page(
			&mut self.connection,
			tenant,
			offset,
		)
		.await
		.map_err(Into::into)
	}
}
struct Scope {
	access: Access,
}
impl Scope {
	async fn finish<T: Send>(
		self,
		result: aidash_application::Result<T>,
	) -> aidash_application::Result<T> {
		self.access
			.finish(result.map_err(Error::from))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl GenerationReads for NativeReads {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn pages(&self) -> aidash_application::Result<Box<dyn GenerationPages>> {
		let lease = self.store.orm_connection()?;
		let connection = lease.handle();
		Ok(Box::new(Pages {
			_lease: lease,
			connection,
		}))
	}
	async fn begin_subject(&self) -> aidash_application::Result<Box<dyn GenerationReadScope>> {
		let Actor::Subject(identity) = &self.actor else {
			return Err(aidash_application::Error::Forbidden);
		};
		Ok(Box::new(Scope {
			access: Access::begin(&self.store, identity).await?,
		}))
	}
	async fn operator_history(
		&self,
		tenant: &str,
		id: Uuid,
	) -> aidash_application::Result<Vec<History>> {
		Ok(crate::database::query_as(&history_query())
			.bind(tenant)
			.bind(id)
			.fetch_all(&self.store.pool)
			.await?)
	}
	async fn operator_usage(&self, tenant: &str, id: Uuid) -> aidash_application::Result<Usage> {
		crate::database::query_as(&usage_query())
			.bind(tenant)
			.bind(id)
			.fetch_optional(&self.store.pool)
			.await?
			.ok_or(aidash_application::Error::Forbidden)
	}
	async fn operator_specification(
		&self,
		tenant: &str,
		id: Uuid,
	) -> aidash_application::Result<Value> {
		crate::database::native::query_scalar(&spec_query())
			.bind(tenant)
			.bind(id)
			.scalar_optional(&self.store.pool)
			.await?
			.ok_or(aidash_application::Error::Forbidden)
	}
}
#[async_trait]
impl GenerationVisibility for Scope {
	fn inherited_lease(&self) -> bool {
		self.access.inherited_lease
	}
	fn context(&mut self, value: Value) {
		self.access.context = value;
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> aidash_application::Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn task(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> aidash_application::Result<Option<Task>> {
		GenerationVisibility::task(
			&mut crate::bootstrap::generation_visibility_scope(&mut self.access),
			id,
			workspace,
		)
		.await
	}
	async fn task_visible(&mut self, task: &Task) -> aidash_application::Result<bool> {
		self.access.task_visible(task).await.map_err(Into::into)
	}
	async fn decide(
		&mut self,
		resource: &Resource,
		action: &str,
	) -> aidash_application::Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl GenerationReadScope for Scope {
	async fn job(&mut self, tenant: &str, id: Uuid) -> aidash_application::Result<Option<Request>> {
		let access = &mut self.access;
		let job: Option<Request> = {
			let query_bind_1 = &tenant;
			let query_bind_2 = id;
			crate::database::query_as(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
					.from(Alias::new("generation_requests"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ? AND id = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
		};

		Ok(job)
	}
	async fn history(
		&mut self,
		tenant: &str,
		id: Uuid,
	) -> aidash_application::Result<Vec<History>> {
		Ok(crate::database::query_as(&history_query())
			.bind(tenant)
			.bind(id)
			.fetch_all(&mut **self.access.tx)
			.await?)
	}
	async fn usage(&mut self, tenant: &str, id: Uuid) -> aidash_application::Result<Usage> {
		Ok(crate::database::query_as(&usage_query())
			.bind(tenant)
			.bind(id)
			.fetch_one(&mut **self.access.tx)
			.await?)
	}
	async fn specification(&mut self, tenant: &str, id: Uuid) -> aidash_application::Result<Value> {
		Ok(crate::database::native::query_scalar(&spec_query())
			.bind(tenant)
			.bind(id)
			.scalar_one(&mut **self.access.tx)
			.await?)
	}
	async fn finish_requests(
		self: Box<Self>,
		result: aidash_application::Result<Vec<Request>>,
	) -> aidash_application::Result<Vec<Request>> {
		(*self).finish(result).await
	}
	async fn finish_history(
		self: Box<Self>,
		result: aidash_application::Result<Vec<History>>,
	) -> aidash_application::Result<Vec<History>> {
		(*self).finish(result).await
	}
	async fn finish_usage(
		self: Box<Self>,
		result: aidash_application::Result<Usage>,
	) -> aidash_application::Result<Usage> {
		(*self).finish(result).await
	}
	async fn finish_specification(
		self: Box<Self>,
		result: aidash_application::Result<Spec>,
	) -> aidash_application::Result<Spec> {
		(*self).finish(result).await
	}
}

fn history_query() -> String {
	Query::select()
		.expr(SimpleExpr::from(Expr::col(ColumnRef::table_asterisk(
			Alias::new("h"),
		))))
		.from_as(Alias::new("generation_history"), Alias::new("h"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("generation_requests"), Alias::new("r")),
			Expr::cust("r.id = h.request_id"),
		)
		.and_where(Expr::cust("r.tenant = $1 AND r.id = $2"))
		.order_by_expr(
			SimpleExpr::from(Expr::col((Alias::new("h"), Alias::new("sequence")))),
			Order::Asc,
		)
		.to_string(PostgresQueryBuilder)
}

fn usage_query() -> String {
	Query::select()
		.expr(SimpleExpr::from(Expr::col((
			Alias::new("b"),
			Alias::new("token_limit"),
		))))
		.expr(SimpleExpr::from(Expr::col((
			Alias::new("b"),
			Alias::new("used_tokens"),
		))))
		.expr(SimpleExpr::from(Expr::col((
			Alias::new("b"),
			Alias::new("compaction_call_limit"),
		))))
		.expr(SimpleExpr::from(Expr::col((
			Alias::new("b"),
			Alias::new("compaction_calls"),
		))))
		.expr(SimpleExpr::from(Expr::col((
			Alias::new("b"),
			Alias::new("embedding_calls"),
		))))
		.expr(SimpleExpr::from(Expr::col((
			Alias::new("b"),
			Alias::new("embedding_call_limit"),
		))))
		.expr_as(
			Expr::cust("(SELECT COUNT(*) FROM generation_usage AS u WHERE u.request_id = r.id) + (SELECT COUNT(*) FROM generation_remote_usage AS u WHERE u.request_id = r.id AND u.purpose='inference' AND u.state<>'RELEASED')"),
			Alias::new("inference_attempts"),
		)
		.from_as(Alias::new("generation_requests"), Alias::new("r"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("generation_budgets"), Alias::new("b")),
			Expr::cust("b.request_id = r.id"),
		)
		.and_where(Expr::cust("r.tenant = $1 AND r.id = $2"))
		.to_string(PostgresQueryBuilder)
}

fn spec_query() -> String {
	Query::select()
		.expr(SimpleExpr::from(Expr::col((
			Alias::new("h"),
			Alias::new("spec"),
		))))
		.from_as(Alias::new("generation_requests"), Alias::new("r"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("generation_policy_history"), Alias::new("h")),
			Expr::cust(
				"h.tenant = r.tenant AND h.policy_id = r.policy_id AND h.revision = r.policy_revision",
			),
		)
		.and_where(Expr::cust("r.tenant = $1 AND r.id = $2"))
		.to_string(PostgresQueryBuilder)
}
