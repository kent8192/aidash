//! PostgreSQL adapters share worker pools, visibility leases and authorized execution.
use crate::{Error, Result, harness::Harness, transactions::gate::ReadLease};
use aidash_application::ports::activation::*;
use aidash_domain::{
	Run,
	activation::{Obligation, QuarantineReason},
	run_state::RawRun,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{QueryRow, execution::convert_values},
};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockBehavior, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;

pub(crate) mod durable;
pub(crate) mod scheduling;

pub(crate) struct Repository {
	pub harness: Harness,
}

pub(super) fn error(error: Error) -> aidash_application::Error {
	match error {
		Error::External(reason) => aidash_application::Error::External(reason),
		error => error.into(),
	}
}

#[async_trait]
impl ActivationRepository for Repository {
	fn node_id(&self) -> &str {
		&self.harness.federation.store.node_id
	}
	fn lease_seconds(&self) -> i32 {
		self.harness.federation.config.lease_seconds
	}
	async fn visibility(&self) -> aidash_application::Result<Box<dyn VisibilityScope>> {
		Ok(Box::new(Visibility {
			harness: self.harness.clone(),
			visibility: ReadLease::begin(&self.harness.federation.store)
				.await
				.map_err(error)?,
		}))
	}
	async fn claim_scope(&self) -> aidash_application::Result<Box<dyn ClaimScope>> {
		Ok(Box::new(scheduling::Scope {
			tx: self
				.harness
				.federation
				.store
				.database()
				.begin()
				.await
				.map_err(Error::from)
				.map_err(error)?,
		}))
	}
	async fn recover(&self) -> aidash_application::Result<Option<(Run, Uuid)>> {
		durable::recover(&self.harness.federation.store, self.lease_seconds())
			.await
			.map_err(error)
	}
	async fn reconcile(&self) -> aidash_application::Result<u64> {
		let visibility = ReadLease::begin(&self.harness.federation.store)
			.await
			.map_err(error)?;
		let count = durable::reconcile(&self.harness.federation.store)
			.await
			.map_err(error)?;
		drop(visibility);
		Ok(count)
	}
	async fn publish_batch(&self, token: Uuid) -> aidash_application::Result<Vec<Obligation>> {
		durable::publish_batch(&self.harness.federation.store, token)
			.await
			.map_err(error)
	}
	async fn published(&self, row: &Obligation, token: Uuid) -> aidash_application::Result<()> {
		durable::published(&self.harness.federation.store, row, token)
			.await
			.map_err(error)
	}
	async fn quarantine(
		&self,
		payload: &[u8],
		reason: QuarantineReason,
		sequence: Option<u64>,
	) -> aidash_application::Result<()> {
		durable::quarantine(
			&self.harness.federation.store,
			payload,
			reason.as_str(),
			sequence,
		)
		.await
		.map_err(error)
	}
	async fn observe(&self) -> aidash_application::Result<()> {
		durable::observe(&self.harness.federation.store)
			.await
			.map_err(error)
	}
}

struct Visibility {
	harness: Harness,
	visibility: ReadLease,
}
#[async_trait]
impl VisibilityScope for Visibility {
	async fn suspend(&mut self) -> aidash_application::Result<()> {
		self.visibility.suspend().await.map_err(error)
	}
	async fn advance(self: Box<Self>, run: Run, token: Uuid) -> aidash_application::Result<()> {
		let Self {
			harness,
			visibility,
		} = *self;
		harness
			.advance_leased(run, token, visibility)
			.await
			.map(|_| ())
			.map_err(error)
	}
}

#[async_trait]
impl ClaimScope for scheduling::Scope<Box<dyn TransactionExecutor>> {
	async fn read_run(&mut self, id: Uuid) -> aidash_application::Result<RawRun> {
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		scheduling::raw(
			self.tx
				.fetch_one(&sql, convert_values(values))
				.await
				.map_err(Error::from)?,
		)
		.map_err(error)
	}
	async fn lock_run(&mut self, id: Uuid) -> aidash_application::Result<Option<RawRun>> {
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::Nowait)
			.build(PostgresQueryBuilder);
		self.tx
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(|e| {
				if e.database_error()
					.is_some_and(|d| d.code() == Some("55P03"))
				{
					aidash_application::Error::TransactionPending
				} else {
					error(e.into())
				}
			})?
			.map(scheduling::raw)
			.transpose()
			.map_err(error)
	}
	async fn lock_obligation(
		&mut self,
		id: Uuid,
	) -> aidash_application::Result<Option<Obligation>> {
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("run_activations"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		self.tx
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(Error::from)?
			.map(|row| durable::obligation(QueryRow::from_backend_row(row).data))
			.transpose()
			.map_err(error)
	}
	async fn newer_transition(
		&mut self,
		row: &Obligation,
		run: &RawRun,
	) -> aidash_application::Result<bool> {
		let newer = Query::select()
			.expr(Expr::value(1_i64))
			.from(Alias::new("run_activations"))
			.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
			.and_where(Expr::col("generation").gt(Expr::value(row.generation)))
			.and_where(Expr::col("run_revision").gte(Expr::value(run.revision)))
			.to_owned();
		let (sql, values) = Query::select()
			.expr_as(Expr::exists(newer), Alias::new("newer"))
			.build(PostgresQueryBuilder);
		self.tx
			.fetch_one(&sql, convert_values(values))
			.await
			.map_err(Error::from)?
			.get("newer")
			.map_err(FrameworkError::from)
			.map_err(Error::from)
			.map_err(error)
	}
	async fn settle(&mut self, id: Uuid, reason: &str) -> aidash_application::Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new("run_activations"))
			.value(Alias::new("state"), "settled")
			.value(Alias::new("due_at"), None::<DateTime<Utc>>)
			.value(Alias::new("disposition"), reason)
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		self.tx
			.execute(&sql, convert_values(values))
			.await
			.map_err(Error::from)?;
		Ok(())
	}
	async fn record_claim(
		&mut self,
		id: Uuid,
		run: &Run,
		token: Uuid,
		seconds: i32,
	) -> aidash_application::Result<()> {
		durable::record_claim(self.tx.as_mut(), id, run, token, seconds, "notification")
			.await
			.map_err(error)
	}
	async fn defer(
		&mut self,
		id: Uuid,
		due: Option<DateTime<Utc>>,
	) -> aidash_application::Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new("run_activations"))
			.value(Alias::new("state"), "deferred")
			.value(Alias::new("due_at"), due)
			.value(Alias::new("disposition"), "owner_deadline_or_unblock")
			.value_expr(
				Alias::new("publication_epoch"),
				Expr::col("publication_epoch").add(Expr::value(1_i64)),
			)
			.value(Alias::new("published_at"), None::<DateTime<Utc>>)
			.value(Alias::new("publish_until"), None::<DateTime<Utc>>)
			.value(Alias::new("publish_token"), None::<Uuid>)
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		self.tx
			.execute(&sql, convert_values(values))
			.await
			.map_err(Error::from)?;
		Ok(())
	}
	async fn commit(self: Box<Self>) -> aidash_application::Result<()> {
		self.tx.commit().await.map_err(Error::from).map_err(error)
	}
}
