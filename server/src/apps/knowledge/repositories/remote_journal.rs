//! Native journal statements retain row locks, database clock, conditional dispatch and attempt fences.
use crate::{Error as NativeError, Result as NativeResult, store::Store};
use aidash_application::{
	Error, Result,
	ports::semantic::remote_journal::{JournalRepository, JournalScope},
};
use aidash_domain::semantic::{
	Failure,
	remote::{
		Operation, Receipt, SourceRead,
		journal::{Attempt, Record as State},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
fn a(name: &str) -> Alias {
	Alias::new(name)
}
fn sql(q: reinhardt::query::SelectStatement) -> String {
	q.to_string(PostgresQueryBuilder)
}
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct Record {
	pub id: Uuid,
	pub home_node: String,
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub digest: String,
	pub binding: Value,
	pub state: String,
	pub cycle: i32,
	pub failures: i32,
	pub attempt_id: Option<Uuid>,
	pub fence: i64,
	pub lease_until: Option<DateTime<Utc>>,
	pub next_attempt: Option<DateTime<Utc>>,
	pub error: Option<String>,
	pub receipt: Option<Value>,
}
impl From<Record> for State {
	fn from(row: Record) -> Self {
		Self {
			id: row.id,
			home_node: row.home_node,
			grant_id: row.grant_id,
			admission_id: row.admission_id,
			digest: row.digest,
			binding: row.binding,
			state: row.state,
			cycle: row.cycle,
			failures: row.failures,
			attempt_id: row.attempt_id,
			fence: row.fence,
			lease_until: row.lease_until,
			next_attempt: row.next_attempt,
			error: row.error,
			receipt: row.receipt,
		}
	}
}
impl From<State> for Record {
	fn from(row: State) -> Self {
		Self {
			id: row.id,
			home_node: row.home_node,
			grant_id: row.grant_id,
			admission_id: row.admission_id,
			digest: row.digest,
			binding: row.binding,
			state: row.state,
			cycle: row.cycle,
			failures: row.failures,
			attempt_id: row.attempt_id,
			fence: row.fence,
			lease_until: row.lease_until,
			next_attempt: row.next_attempt,
			error: row.error,
			receipt: row.receipt,
		}
	}
}
pub(crate) struct Repository<'a> {
	pub(crate) store: &'a Store,
}
pub(crate) enum Transaction<'a, 'tx> {
	Owned(sqlx::Transaction<'static, sqlx::Postgres>),
	Borrowed(&'a mut sqlx::Transaction<'tx, sqlx::Postgres>),
}
pub(crate) struct Scope<'a, 'tx> {
	pub(crate) store: &'a Store,
	pub(crate) transaction: Transaction<'a, 'tx>,
}
impl Scope<'_, '_> {
	fn connection(&mut self) -> &mut sqlx::PgConnection {
		match &mut self.transaction {
			Transaction::Owned(tx) => tx,
			Transaction::Borrowed(tx) => tx,
		}
	}
}
#[async_trait]
impl JournalRepository for Repository<'_> {
	async fn begin(&self) -> Result<Box<dyn JournalScope + '_>> {
		let tx = self.store.pool.begin().await.map_err(NativeError::from)?;
		Ok(Box::new(Scope {
			store: self.store,
			transaction: Transaction::Owned(tx),
		}))
	}
	async fn bound(&self, id: Uuid) -> Result<State> {
		let result: NativeResult<Record> = async {
			let store = self.store;
			let record: Record = sqlx::query_as(&sql(Query::select()
				.column(Asterisk)
				.from(a("semantic_remote_operations"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(Expr::cust("$1")),
				)
				.to_owned()))
			.bind(id)
			.fetch_optional(&store.pool)
			.await?
			.ok_or(NativeError::Forbidden)?;
			Ok(record)
		}
		.await;
		result.map(Into::into).map_err(Into::into)
	}
	async fn expire_cached(&self, id: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			let store = self.store;
			{
				let query_bind_1 = id;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_operations"))
						.value(a("state"), "PENDING")
						.value_expr(a("receipt"), Expr::cust("NULL"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND state='READY')".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&store.pool)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl JournalScope for Scope<'_, '_> {
	async fn insert_binding(
		&mut self,
		operation: &Operation,
		digest: &str,
		value: &Value,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			sqlx::query(&format!(
				"{} ON CONFLICT DO NOTHING",
				Query::insert()
					.into_table(a("semantic_remote_operations"))
					.columns(
						[
							"id",
							"home_node",
							"grant_id",
							"admission_id",
							"digest",
							"binding",
						]
						.map(a),
					)
					.from_subquery(
						Query::select()
							.expr(Expr::cust("$1"))
							.expr(Expr::cust("$2"))
							.expr(Expr::cust("$3"))
							.expr(Expr::cust("$4"))
							.expr(Expr::cust("$5"))
							.expr(Expr::cust("$6"))
							.to_owned()
					)
					.to_owned()
					.to_string(PostgresQueryBuilder)
			))
			.bind(operation.id)
			.bind(&operation.home_node)
			.bind(operation.grant_id)
			.bind(operation.admission_id)
			.bind(&digest)
			.bind(&value)
			.execute(&mut *tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn shared(&mut self, id: Uuid) -> Result<State> {
		let result: NativeResult<State> = async {
			let tx = self.connection();
			let record: Record = sqlx::query_as(&sql(Query::select()
				.column(Asterisk)
				.from(a("semantic_remote_operations"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(Expr::cust("$1")),
				)
				.lock(LockType::Share)
				.to_owned()))
			.bind(id)
			.fetch_one(&mut *tx)
			.await?;
			Ok(record.into())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn locked(&mut self, id: Uuid) -> Result<State> {
		let result: NativeResult<State> = async {
			let tx = self.connection();
			let record: Record = sqlx::query_as(&sql(Query::select()
				.column(Asterisk)
				.from(a("semantic_remote_operations"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(Expr::cust("$1")),
				)
				.lock(LockType::Update)
				.to_owned()))
			.bind(id)
			.fetch_one(&mut *tx)
			.await?;
			Ok(record.into())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn clock(&mut self) -> Result<DateTime<Utc>> {
		let result: NativeResult<DateTime<Utc>> = async {
			let tx = self.connection();
			let now: DateTime<Utc> = sqlx::query_scalar(&sql(Query::select()
				.expr(Expr::cust("CLOCK_TIMESTAMP()"))
				.to_owned()))
			.fetch_one(&mut *tx)
			.await?;
			Ok(now)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn expire_attempt(&mut self, previous: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			{
				let query_bind_1 = previous;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_attempts"))
						.value_expr(
							a("state"),
							Expr::cust(
								"CASE WHEN state = 'RESERVING' THEN 'ABORTED' ELSE 'UNCERTAIN' END",
							),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND state IN ('RESERVING','DISPATCHED'))".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn wait_expired(
		&mut self,
		id: Uuid,
		failures: i32,
		delay: Option<i64>,
		state: &str,
		error: &str,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			{
				let query_bind_1 = id;
				let query_bind_2 = state;
				let query_bind_3 = delay.map(|s| s as f64);
				let query_bind_4 = error;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_operations"))
						.value_expr(
							a("state"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.value(a("failures"), failures)
						.value_expr(
							a("fence"),
							Expr::col(a("fence")).add(reinhardt::query::Expr::value(1)),
						)
						.value_expr(a("lease_until"), Expr::cust("NULL"))
						.value_expr(
							a("next_attempt"),
							SimpleExpr::CustomWithExpr(
								"(CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => ?))".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							),
						)
						.value_expr(
							a("error"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							),
						)
						.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn activate(&mut self, id: Uuid, attempt: &Attempt, record: &State) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			{
				let query_bind_1 = id;
				let query_bind_2 = attempt.id;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_operations"))
						.value(a("state"), "ACTIVE")
						.value(a("fence"), attempt.fence)
						.value_expr(
							a("attempt_id"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.value_expr(a("next_attempt"), Expr::cust("NULL"))
						.value_expr(
							a("lease_until"),
							Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '120 seconds'"),
						)
						.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			{
				let query_bind_1 = attempt.id;
				let query_bind_2 = id;
				sqlx::query(
					&Query::insert()
						.into_table(a("semantic_remote_attempts"))
						.columns(["id", "operation_id", "fence", "cycle", "state"].map(a))
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(Expr::value(attempt.fence))
								.expr(Expr::value(record.cycle))
								.expr(Expr::value("RESERVING"))
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn current(&mut self, attempt: &Attempt) -> Result<State> {
		let result: NativeResult<Record> = async {
			let tx = self.connection();
			sqlx::query_as(&sql(Query::select()
				.column(Asterisk)
				.from(a("semantic_remote_operations"))
				.and_where(Expr::cust(
					"id=$1 AND attempt_id=$2 AND fence=$3 AND state='ACTIVE' AND lease_until > CLOCK_TIMESTAMP()",
				))
				.lock(LockType::Update)
				.to_owned()))
			.bind(attempt.operation_id)
			.bind(attempt.id)
			.bind(attempt.fence)
			.fetch_optional(&mut *tx)
			.await?
			.ok_or(NativeError::RemoteSemantic(Failure::Pending))
		}
		.await;
		result.map(Into::into).map_err(Into::into)
	}
	async fn dispatched(&mut self, attempt: &Attempt, reservations: &Value) -> Result<u64> {
		let result: NativeResult<u64> = async {
			let tx = self.connection();
			let changed = {
				let query_bind_1 = attempt.id;
				let query_bind_2 = reservations;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_attempts"))
						.value(a("state"), "DISPATCHED")
						.value_expr(
							a("reservations"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.value_expr(a("dispatched_at"), Expr::cust("CLOCK_TIMESTAMP()"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND state='RESERVING')".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			}
			.rows_affected();
			Ok(changed)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn record_source(&mut self, receipt: &Receipt, source: &SourceRead) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			sqlx::query(&format!(
				"{} ON CONFLICT DO NOTHING",
				Query::insert()
					.into_table(a("semantic_remote_reads"))
					.columns(
						[
							"grant_id",
							"admission_id",
							"entry_id",
							"revision",
							"content_digest",
						]
						.map(a),
					)
					.from_subquery(
						Query::select()
							.expr(Expr::cust("$1"))
							.expr(Expr::cust("$2"))
							.expr(Expr::cust("$3"))
							.expr(Expr::cust("$4"))
							.expr(Expr::cust("$5"))
							.to_owned()
					)
					.to_owned()
					.to_string(PostgresQueryBuilder)
			))
			.bind(receipt.grant_id)
			.bind(receipt.admission_id)
			.bind(source.entry_id)
			.bind(source.revision)
			.bind(&source.content_digest)
			.execute(&mut *tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn complete_operation(&mut self, receipt: &Receipt) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			{
				let query_bind_1 = receipt.operation_id;
				let query_bind_2 = serde_json::to_value(receipt)?;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_operations"))
						.value(a("state"), "READY")
						.value_expr(
							a("receipt"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.value_expr(a("lease_until"), Expr::cust("NULL"))
						.value_expr(a("error"), Expr::cust("NULL"))
						.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn complete_attempt(&mut self, attempt: &Attempt) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			{
				let query_bind_1 = attempt.id;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_attempts"))
						.value(a("state"), "COMPLETED")
						.value_expr(a("completed_at"), Expr::cust("CLOCK_TIMESTAMP()"))
						.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn fail_operation(
		&mut self,
		record: &State,
		failures: i32,
		delay: Option<i64>,
		state: &str,
		error: &str,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			{
				let query_bind_1 = record.id;
				let query_bind_2 = error;
				let query_bind_3 = delay.map(|s| s as f64);
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_operations"))
						.value(a("state"), state)
						.value(a("failures"), failures)
						.value_expr(
							a("error"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.value_expr(a("lease_until"), Expr::cust("NULL"))
						.value_expr(
							a("next_attempt"),
							SimpleExpr::CustomWithExpr(
								"(CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => ?))".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							),
						)
						.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn fail_attempt(&mut self, attempt: &Attempt, error: &str) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			{
				let query_bind_1 = attempt.id;
				let query_bind_2 = error;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_attempts"))
						.value_expr(
							a("state"),
							Expr::cust(
								"CASE WHEN state='RESERVING' THEN 'ABORTED' ELSE 'UNCERTAIN' END",
							),
						)
						.value_expr(
							a("error"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND state IN ('RESERVING','DISPATCHED'))".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn resume_records(&mut self, grant: Uuid, admission: Uuid) -> Result<Vec<State>> {
		let result: NativeResult<Vec<State>> = async {
			let tx = self.connection();
			let records: Vec<Record> = sqlx::query_as(&sql(Query::select()
				.column(Asterisk)
				.from(a("semantic_remote_operations"))
				.and_where(Expr::cust("grant_id=$1 AND admission_id=$2"))
				.order_by(a("id"), reinhardt::query::Order::Asc)
				.lock(LockType::Update)
				.to_owned()))
			.bind(grant)
			.bind(admission)
			.fetch_all(&mut *tx)
			.await?;
			Ok(records.into_iter().map(Into::into).collect())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn resume_record(&mut self, record: &State) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = self.connection();
			{
				let query_bind_1 = record.id;
				sqlx::query(
					&Query::update()
						.table(a("semantic_remote_operations"))
						.value_expr(
							a("cycle"),
							Expr::col(a("cycle")).add(reinhardt::query::Expr::value(1)),
						)
						.value(a("state"), "PENDING")
						.value(a("failures"), 0)
						.value_expr(a("next_attempt"), Expr::cust("NULL"))
						.value_expr(a("lease_until"), Expr::cust("NULL"))
						.value_expr(a("error"), Expr::cust("NULL"))
						.and_where(reinhardt::query::SimpleExpr::from(Expr::col(a("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		let store = self.store;
		match &mut self.transaction {
			Transaction::Owned(tx) => store.event(tx, Some(workspace), kind, data).await,
			Transaction::Borrowed(tx) => store.event(tx, Some(workspace), kind, data).await,
		}
		.map(|_| ())
		.map_err(Into::into)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		match self.transaction {
			Transaction::Owned(tx) => tx
				.commit()
				.await
				.map_err(NativeError::from)
				.map_err(Into::into),
			Transaction::Borrowed(_) => {
				Err(Error::External("journal commit scope invariant".into()))
			}
		}
	}
}
