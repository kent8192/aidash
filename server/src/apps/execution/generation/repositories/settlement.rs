//! Native settlement retains attempt serialization and atomic bounded refunds.
use crate::{Error, store::Store};
use aidash_application::{
	Result,
	ports::generation::settlement::{
		GenerationSettlementRepository, GenerationSettlementScope, GenerationSettlementSession,
	},
};
use aidash_domain::generation::{
	remote::{Attempt, Purpose, ReservedCharge},
	requests::Request,
};
use async_trait::async_trait;
use reinhardt::query::ColumnRef::Asterisk;
use reinhardt::query::{
	Alias, Expr, LockType, OnConflict, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(crate) struct NativeSettlementRepository {
	pub store: Store,
}
pub(crate) struct NativeSettlementScope<'a, 'db> {
	pub transaction: &'a mut Transaction<'db, Postgres>,
}
struct Session {
	transaction: Transaction<'static, Postgres>,
}
impl Session {
	fn scope(&mut self) -> NativeSettlementScope<'_, 'static> {
		NativeSettlementScope {
			transaction: &mut self.transaction,
		}
	}
}

#[async_trait]
impl GenerationSettlementRepository for NativeSettlementRepository {
	async fn begin(&self) -> Result<Box<dyn GenerationSettlementSession>> {
		Ok(Box::new(Session {
			transaction: self.store.pool.begin().await.map_err(Error::from)?,
		}))
	}
}
#[async_trait]
impl GenerationSettlementSession for Session {
	async fn commit(self: Box<Self>) -> Result<()> {
		self.transaction.commit().await.map_err(Error::from)?;
		Ok(())
	}
}
#[async_trait]
impl GenerationSettlementScope for Session {
	async fn lock_attempt(&mut self, attempt: Uuid, digest: &str) -> Result<Attempt> {
		self.scope().lock_attempt(attempt, digest).await
	}
	async fn reservations(&mut self, attempt: Uuid, digest: &str) -> Result<Vec<ReservedCharge>> {
		self.scope().reservations(attempt, digest).await
	}
	async fn refund_budget(
		&mut self,
		id: Uuid,
		refund: i64,
		purpose: Purpose,
		release_call: bool,
	) -> Result<u64> {
		self.scope()
			.refund_budget(id, refund, purpose, release_call)
			.await
	}
	async fn request(&mut self, id: Uuid) -> Result<Request> {
		self.scope().request(id).await
	}
	async fn refund_policy(
		&mut self,
		job: &Request,
		refund: i64,
		purpose: Purpose,
		release_call: bool,
	) -> Result<u64> {
		self.scope()
			.refund_policy(job, refund, purpose, release_call)
			.await
	}
	async fn settle_reservation(
		&mut self,
		id: Uuid,
		attempt: Uuid,
		state: &str,
		reported: Option<i64>,
	) -> Result<()> {
		self.scope()
			.settle_reservation(id, attempt, state, reported)
			.await
	}
	async fn save_finalization(&mut self, attempt: Uuid, result: &Value) -> Result<()> {
		self.scope().save_finalization(attempt, result).await
	}
}

#[async_trait]
impl GenerationSettlementScope for NativeSettlementScope<'_, '_> {
	async fn lock_attempt(&mut self, attempt: Uuid, digest: &str) -> Result<Attempt> {
		let tx = &mut *self.transaction;

		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("generation_remote_finalizations"))
				.columns(["attempt_id", "digest"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::cust("$1"))
						.expr(Expr::cust("$2"))
						.to_owned(),
				)
				.on_conflict(
					OnConflict::column(Alias::new("attempt_id"))
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(attempt)
		.bind(digest)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;
		let (saved_digest, result): (String, Option<Value>) = {
			let query_bind_1 = attempt;
			sqlx::query_as(
				&Query::select()
					.columns(["digest", "result"].map(Alias::new))
					.from(Alias::new("generation_remote_finalizations"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await
			.map_err(Error::from)?
		};
		Ok(Attempt {
			digest: saved_digest,
			result,
		})
	}
	async fn reservations(&mut self, attempt: Uuid, digest: &str) -> Result<Vec<ReservedCharge>> {
		let tx = &mut *self.transaction;
		let rows: Vec<(Uuid, String, Option<i64>)> = {
			let query_bind_1 = attempt;
			let query_bind_2 = digest;
			sqlx::query_as(
				&Query::select()
					.columns(["request_id", "state", "reported_tokens"].map(Alias::new))
					.from(Alias::new("generation_remote_usage"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=? AND digest=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.order_by(Alias::new("request_id"), Order::Asc)
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **tx)
			.await
			.map_err(Error::from)?
		};
		Ok(rows
			.into_iter()
			.map(|(request_id, state, reported_tokens)| ReservedCharge {
				request_id,
				state,
				reported_tokens,
			})
			.collect())
	}
	async fn refund_budget(
		&mut self,
		id: Uuid,
		refund: i64,
		purpose: Purpose,
		release_call: bool,
	) -> Result<u64> {
		let tx = &mut *self.transaction;

		let mut q = Query::update();
		q.table(Alias::new("generation_budgets"))
			.value_expr(Alias::new("used_tokens"), Expr::cust("used_tokens-$2"))
			.and_where(Expr::cust("request_id=$1 AND used_tokens >= $2"));
		if release_call && let Some((counter, _)) = counter(purpose) {
			q.value_expr(Alias::new(counter), Expr::cust(format!("{counter}-1")))
				.and_where(Expr::cust(format!("{counter}>0")));
		}
		let changed = sqlx::query(&q.to_string(PostgresQueryBuilder))
			.bind(id)
			.bind(refund)
			.execute(&mut **tx)
			.await
			.map_err(Error::from)?;
		Ok(changed.rows_affected())
	}

	async fn request(&mut self, id: Uuid) -> Result<Request> {
		let tx = &mut *self.transaction;
		let job: Request = {
			let query_bind_1 = id;
			crate::database::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("generation_requests"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await
			.map_err(Error::from)?
		};
		Ok(job)
	}
	async fn refund_policy(
		&mut self,
		job: &Request,
		refund: i64,
		purpose: Purpose,
		release_call: bool,
	) -> Result<u64> {
		let tx = &mut *self.transaction;
		let mut q = Query::update();
		q.table(Alias::new("generation_policies"))
			.value_expr(
				Alias::new("allocated_tokens"),
				Expr::cust("allocated_tokens-$3"),
			)
			.and_where(Expr::cust("tenant=$1 AND id=$2 AND allocated_tokens >= $3"));
		if release_call && let Some((_, limit)) = counter(purpose) {
			let allocation = if limit == "embedding_call_limit" {
				"allocated_embedding_calls"
			} else {
				"allocated_compaction_calls"
			};
			q.value_expr(
				Alias::new(allocation),
				Expr::cust(format!("{allocation}-1")),
			)
			.and_where(Expr::cust(format!("{allocation}>0")));
		}
		let changed = sqlx::query(&q.to_string(PostgresQueryBuilder))
			.bind(&job.tenant)
			.bind(&job.policy_id)
			.bind(refund)
			.execute(&mut **tx)
			.await
			.map_err(Error::from)?;
		Ok(changed.rows_affected())
	}

	async fn settle_reservation(
		&mut self,
		id: Uuid,
		attempt: Uuid,
		state: &str,
		reported: Option<i64>,
	) -> Result<()> {
		let tx = &mut *self.transaction;
		{
			let query_bind_1 = id;
			let query_bind_2 = attempt;
			let query_bind_3 = state;
			let query_bind_4 = reported;
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_remote_usage"))
					.value_expr(
						Alias::new("state"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(
						Alias::new("reported_tokens"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(request_id=? AND attempt_id=? AND state='RESERVED')".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await
			.map_err(Error::from)?
		};
		Ok(())
	}
	async fn save_finalization(&mut self, attempt: Uuid, result: &Value) -> Result<()> {
		let tx = &mut *self.transaction;
		{
			let query_bind_1 = attempt;
			let query_bind_2 = json!(result);
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_remote_finalizations"))
					.value_expr(
						Alias::new("result"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await
			.map_err(Error::from)?
		};
		Ok(())
	}
}

pub(crate) fn counter(purpose: Purpose) -> Option<(&'static str, &'static str)> {
	match purpose {
		Purpose::Embedding => Some(("embedding_calls", "embedding_call_limit")),
		Purpose::Compaction => Some(("compaction_calls", "compaction_call_limit")),
		Purpose::Inference => None,
	}
}
