//! Thread ownership queries keep their existing lock modes, ordering and native transaction.
use super::{NativeReads, Reads};
use crate::Result as NativeResult;
use aidash_application::{
	Result,
	authorization::Snapshot,
	ports::authorization::{
		lease::AuthorizationLease,
		threads::{RunInteractionScope, ThreadAuthorityScope, ThreadOwner, ThreadResourceScope},
	},
};
use aidash_domain::{
	Run, RunMetadata,
	policy::{Decision, Evaluation, Resource},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder,
	Query, QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
impl AuthorizationLease for Reads<'_> {
	fn snapshot(&self) -> &Snapshot {
		&self.access.snapshot
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn environment(&self) -> &Value {
		self.access.environment()
	}
	async fn record(&mut self, records: &[(Evaluation, Decision)]) -> Result<()> {
		self.access.record(records).await.map_err(Into::into)
	}
}
/// The Thread's owner and its Duty, read from one row of
/// `authorization_threads` in one statement (#1008): `duty_id` is a scalar
/// subquery, so the locking clause applies to the authority row only.
fn owner_query() -> reinhardt::query::SelectStatement {
	let threads = || Alias::new("authorization_threads");
	let mut query = Query::select();
	query
		.column(Alias::new("owner_subject"))
		.expr_as(
			crate::apps::workspaces::repositories::duties::own_duty(
				Expr::col((threads(), Alias::new("thread_id"))).into(),
				Expr::col((threads(), Alias::new("tenant"))).into(),
			),
			Alias::new("duty_id"),
		)
		.from(threads())
		.and_where(
			crate::apps::identity::repositories::nodes::live_thread_condition(
				Expr::col((threads(), Alias::new("thread_id"))).into(),
				Expr::col((threads(), Alias::new("tenant"))).into(),
			),
		);
	query
}
/// [`owner_query`] for every live Thread of a Tenant, with its id.
fn owners_query() -> reinhardt::query::SelectStatement {
	let mut query = owner_query();
	query
		.column((Alias::new("authorization_threads"), Alias::new("thread_id")))
		.order_by(Alias::new("thread_id"), Order::Asc)
		.lock(LockType::Share);
	query
}
fn owned((owner, duty_id): (String, Option<Uuid>)) -> ThreadOwner {
	ThreadOwner { owner, duty_id }
}
fn owned_rows(rows: Vec<(String, Option<Uuid>, Uuid)>) -> Vec<(Uuid, ThreadOwner)> {
	rows.into_iter()
		.map(|(owner, duty_id, id)| (id, ThreadOwner { owner, duty_id }))
		.collect()
}
#[async_trait]
impl ThreadResourceScope for Reads<'_> {
	fn inherited(&self) -> bool {
		self.access.inherited_lease
	}
	fn context(&self) -> &Value {
		&self.access.context
	}
	fn recheck_reaches(&self, id: Uuid) -> bool {
		self.access.recheck_reaches(id)
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn owner(&mut self, id: Uuid, lock: bool) -> Result<Option<ThreadOwner>> {
		if let Some(owner) = self.access.tx.thread_owner(id, lock) {
			return Ok(Some(owner));
		}
		let result: NativeResult<Option<ThreadOwner>> = async {
			let mut query = owner_query();
			query.and_where(
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
							.eq(Expr::cust("$1")),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
							.eq(Expr::cust("$2")),
					),
			);
			if lock {
				query.lock(LockType::Share);
			}
			let owner = crate::database::native::query_as::<(String, Option<Uuid>)>(
				&query.to_string(PostgresQueryBuilder),
			)
			.columns(&["owner_subject", "duty_id"])
			.bind(id)
			.bind(&self.access.identity.tenant)
			.fetch_optional(&mut **self.access.tx)
			.await?
			.map(owned);
			if let Some(owner) = &owner {
				self.access.tx.remember_thread_owner(id, lock, owner);
			}
			Ok(owner)
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl ThreadAuthorityScope for Reads<'_> {
	async fn locked_owner(&mut self, id: Uuid) -> Result<Option<ThreadOwner>> {
		if let Some(owner) = self.access.tx.thread_owner(id, true) {
			return Ok(Some(owner));
		}
		let result: NativeResult<Option<ThreadOwner>> = async {
			let mut query = owner_query();
			query
				.and_where(
					Condition::all()
						.add(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(id).into()],
								)),
						)
						.add(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(self.access.identity.tenant.clone()).into()],
								),
							),
						),
				)
				.lock(LockType::Share);
			let owner = crate::database::native::query_as::<(String, Option<Uuid>)>(
				&query.to_string(PostgresQueryBuilder),
			)
			.columns(&["owner_subject", "duty_id"])
			.fetch_optional(&mut **self.access.tx)
			.await?
			.map(owned);
			if let Some(owner) = &owner {
				self.access.tx.remember_thread_owner(id, true, owner);
			}
			Ok(owner)
		}
		.await;
		result.map_err(Into::into)
	}
	fn identity(&self) -> (&str, &str) {
		(&self.access.identity.tenant, &self.access.identity.subject)
	}
	async fn thread_node(&mut self, id: Uuid) -> Result<String> {
		self.access
			.thread_node(id)
			.await
			.map(|node| node.as_str().to_owned())
			.map_err(Into::into)
	}
	async fn all_owners(&mut self) -> Result<Vec<(Uuid, ThreadOwner)>> {
		let result: NativeResult<Vec<(Uuid, ThreadOwner)>> = async {
			let mut query = owners_query();
			query.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(self.access.identity.tenant.clone()).into()],
					),
				),
			);
			let rows = crate::database::native::query_as::<(String, Option<Uuid>, Uuid)>(
				&query.to_string(PostgresQueryBuilder),
			)
			.columns(&["owner_subject", "duty_id", "thread_id"])
			.fetch_all(&mut **self.access.tx)
			.await?;
			Ok(owned_rows(rows))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn event_owners(&mut self, selected: Option<Uuid>) -> Result<Vec<(Uuid, ThreadOwner)>> {
		let result: NativeResult<Vec<(Uuid, ThreadOwner)>> = async {
			let mut query = owners_query();
			query.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
					.eq(Expr::cust("$1")),
			);
			if selected.is_some() {
				query.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
						.eq(Expr::cust("$2")),
				);
			}
			let sql = query.to_string(PostgresQueryBuilder);
			let mut query = crate::database::native::query_as::<(String, Option<Uuid>, Uuid)>(&sql)
				.columns(&["owner_subject", "duty_id", "thread_id"])
				.bind(&self.access.identity.tenant);
			if let Some(id) = selected {
				query = query.bind(id);
			}
			let rows = query.fetch_all(&mut **self.access.tx).await?;
			Ok(owned_rows(rows))
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl ThreadResourceScope for NativeReads<'_> {
	fn inherited(&self) -> bool {
		self.access.inherited_lease()
	}
	fn context(&self) -> &Value {
		self.access.thread_context()
	}
	fn recheck_reaches(&self, id: Uuid) -> bool {
		self.access.recheck_reaches(id)
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn owner(&mut self, id: Uuid, lock: bool) -> Result<Option<ThreadOwner>> {
		if let Some(owner) = self.access.tx.thread_owner(id, lock) {
			return Ok(Some(owner));
		}
		let (tx, tenant) = self.access.tenant_transaction();
		let owner =
			crate::apps::identity::models::AuthorizationThread::owner_in(tx, id, tenant, lock)
				.await?;
		if let Some(owner) = &owner {
			self.access.tx.remember_thread_owner(id, lock, owner);
		}
		Ok(owner)
	}
}
#[async_trait]
impl RunInteractionScope for Reads<'_> {
	async fn run(&mut self, id: Uuid) -> Result<Option<Run>> {
		let result: NativeResult<Option<Run>> = async {
			let run: Option<Run> = {
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("task_executions"))
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
				.fetch_optional(&mut **self.access.tx)
				.await?
			};
			Ok(run)
		}
		.await;
		result.map_err(Into::into)
	}
	fn set_context(&mut self, context: Value) {
		self.access.context = context;
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		self.access.run_visible(run).await.map_err(Into::into)
	}
	async fn admitted_here(&mut self, run: &RunMetadata) -> Result<bool> {
		Ok(
			crate::apps::federation::peer::repositories::admissions::for_run(
				self.access.tx.as_mut(),
				run.id,
			)
			.await
			.map_err(aidash_application::Error::from)?
			.is_some(),
		)
	}
}
#[async_trait]
impl RunInteractionScope for NativeReads<'_> {
	async fn run(&mut self, id: Uuid) -> Result<Option<Run>> {
		crate::apps::execution::models::Run::read_in(self.access.tx.as_mut(), id, None)
			.await
			.map_err(Into::into)
	}
	fn set_context(&mut self, context: Value) {
		self.access.set_context(context);
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		self.access.run_visible(run).await.map_err(Into::into)
	}
	async fn admitted_here(&mut self, run: &RunMetadata) -> Result<bool> {
		Ok(
			crate::apps::federation::peer::repositories::admissions::for_run(
				self.access.tx.as_mut(),
				run.id,
			)
			.await
			.map_err(aidash_application::Error::from)?
			.is_some(),
		)
	}
}
