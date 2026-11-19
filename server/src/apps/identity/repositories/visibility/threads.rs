//! Thread ownership queries keep their existing lock modes, ordering and native transaction.
use super::{NativeReads, Reads};
use crate::Result as NativeResult;
use aidash_application::{
	Result,
	authorization::Snapshot,
	ports::authorization::{
		lease::AuthorizationLease,
		threads::{RunInteractionScope, ThreadAuthorityScope, ThreadResourceScope},
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
	async fn owner(&mut self, id: Uuid, lock: bool) -> Result<Option<String>> {
		if let Some(owner) = self.access.tx.thread_owner(id, lock) {
			return Ok(Some(owner));
		}
		let result: NativeResult<Option<String>> = async {
			let query = if !lock {
				Query::select()
					.column(Alias::new("owner_subject"))
					.from(Alias::new("authorization_threads"))
					.and_where(
						crate::apps::identity::repositories::nodes::live_thread_condition(
							Expr::col((
								Alias::new("authorization_threads"),
								Alias::new("thread_id"),
							))
							.into(),
							Expr::col((Alias::new("authorization_threads"), Alias::new("tenant")))
								.into(),
						),
					)
					.and_where(
						Condition::all()
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
									"thread_id",
								)))
								.eq(Expr::cust("$1")),
							)
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
									.eq(Expr::cust("$2")),
							),
					)
					.to_string(PostgresQueryBuilder)
			} else {
				Query::select()
					.column(Alias::new("owner_subject"))
					.from(Alias::new("authorization_threads"))
					.and_where(
						crate::apps::identity::repositories::nodes::live_thread_condition(
							Expr::col((
								Alias::new("authorization_threads"),
								Alias::new("thread_id"),
							))
							.into(),
							Expr::col((Alias::new("authorization_threads"), Alias::new("tenant")))
								.into(),
						),
					)
					.and_where(
						Condition::all()
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
									"thread_id",
								)))
								.eq(Expr::cust("$1")),
							)
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
									.eq(Expr::cust("$2")),
							),
					)
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder)
			};
			let owner: Option<String> = crate::database::native::query_scalar(&query)
				.bind(id)
				.bind(&self.access.identity.tenant)
				.scalar_optional(&mut **self.access.tx)
				.await?;
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
	async fn locked_owner(&mut self, id: Uuid) -> Result<Option<String>> {
		if let Some(owner) = self.access.tx.thread_owner(id, true) {
			return Ok(Some(owner));
		}
		let result: NativeResult<Option<String>> =
			async {
				let owner: Option<String> =
					{
						let query_bind_1 = id;
						let query_bind_2 = &self.access.identity.tenant;
						crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("owner_subject"))
						.from(Alias::new("authorization_threads"))
					.and_where(crate::apps::identity::repositories::nodes::live_thread_condition(
						Expr::col((Alias::new("authorization_threads"), Alias::new("thread_id"))).into(),
						Expr::col((Alias::new("authorization_threads"), Alias::new("tenant"))).into(),
					))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"thread_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"tenant",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **self.access.tx)
				.await?
					};
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
	async fn all_owners(&mut self) -> Result<Vec<(Uuid, String)>> {
		let result: NativeResult<Vec<(Uuid, String)>> =
			async {
				let rows: Vec<(Uuid, String)> =
					{
						let query_bind_1 = &self.access.identity.tenant;
						crate::database::native::query_as(
					&Query::select()
						.column(Alias::new("thread_id"))
						.column(Alias::new("owner_subject"))
						.from(Alias::new("authorization_threads"))
					.and_where(crate::apps::identity::repositories::nodes::live_thread_condition(
						Expr::col((Alias::new("authorization_threads"), Alias::new("thread_id"))).into(),
						Expr::col((Alias::new("authorization_threads"), Alias::new("tenant"))).into(),
					))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.order_by(Alias::new("thread_id"), Order::Asc)
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["thread_id", "owner_subject"])
				.fetch_all(&mut **self.access.tx)
				.await?
					};
				Ok(rows)
			}
			.await;
		result.map_err(Into::into)
	}
	async fn event_owners(&mut self, selected: Option<Uuid>) -> Result<Vec<(Uuid, String)>> {
		let result: NativeResult<Vec<(Uuid, String)>> = async {
			let mut query = Query::select();
			query
				.columns([Alias::new("thread_id"), Alias::new("owner_subject")])
				.from(Alias::new("authorization_threads"))
				.and_where(
					crate::apps::identity::repositories::nodes::live_thread_condition(
						Expr::col((Alias::new("authorization_threads"), Alias::new("thread_id")))
							.into(),
						Expr::col((Alias::new("authorization_threads"), Alias::new("tenant")))
							.into(),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
						.eq(Expr::cust("$1")),
				)
				.order_by(Alias::new("thread_id"), Order::Asc)
				.lock(LockType::Share);
			if selected.is_some() {
				query.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
						.eq(Expr::cust("$2")),
				);
			}
			let sql = query.to_string(PostgresQueryBuilder);
			let mut query = crate::database::native::query_as::<(Uuid, String)>(&sql)
				.columns(&["thread_id", "owner_subject"])
				.bind(&self.access.identity.tenant);
			if let Some(id) = selected {
				query = query.bind(id);
			}
			let rows = query.fetch_all(&mut **self.access.tx).await?;
			Ok(rows)
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
	async fn owner(&mut self, id: Uuid, lock: bool) -> Result<Option<String>> {
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
