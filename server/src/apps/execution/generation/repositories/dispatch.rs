//! PostgreSQL dispatch CAS and owned leases preserve the original query boundaries.
use crate::{Error, federation::Federation, store::Store};
use aidash_application::{
	Result,
	ports::generation::dispatch::{
		DispatchPreparation, DispatchVisibility, GenerationDispatchRepository,
		GenerationDispatchSettlement,
	},
};
use aidash_domain::generation::{
	dispatch::{FinalizeInput, Input, Record},
	remote::{Finalization, Reserved, Usage},
};
use async_trait::async_trait;
use reinhardt::query::ColumnRef::Asterisk;
use reinhardt::query::{
	Alias, Expr, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct StoredDispatch {
	usage: Value,
	digest: String,
	peer_node: String,
	boundary: Value,
	state: String,
	finalization: Option<Value>,
	peer_finalized: bool,
}
impl From<StoredDispatch> for Record {
	fn from(row: StoredDispatch) -> Self {
		Self {
			usage: row.usage,
			digest: row.digest,
			peer_node: row.peer_node,
			boundary: row.boundary,
			state: row.state,
			finalization: row.finalization,
			peer_finalized: row.peer_finalized,
		}
	}
}

pub(crate) struct NativeDispatch {
	pub store: Store,
}
pub(crate) struct NativeSettlement {
	pub runtime: Federation,
}
struct Preparation {
	transaction: Transaction<'static, Postgres>,
}
struct Visibility {
	store: Store,
	lease: crate::transactions::gate::ReadLease,
}

#[async_trait]
impl DispatchPreparation for Preparation {
	async fn insert(&mut self, input: &Input, peer: &str, digest: &str) -> Result<()> {
		let tx = &mut self.transaction;
		sqlx::query(&format!(
			"{} ON CONFLICT DO NOTHING",
			Query::insert()
				.into_table(Alias::new("generation_remote_dispatches"))
				.columns(["attempt_id", "usage", "digest", "peer_node", "boundary"].map(Alias::new))
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
		.bind(input.usage.attempt_id)
		.bind(json!(input.usage))
		.bind(digest)
		.bind(peer)
		.bind(&input.boundary)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;
		Ok(())
	}
	async fn record(&mut self, attempt: Uuid) -> Result<Record> {
		let tx = &mut self.transaction;
		let record: StoredDispatch = {
			let query_bind_1 = attempt;
			sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("generation_remote_dispatches"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await
			.map_err(Error::from)?
		};
		Ok(record.into())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.transaction.commit().await.map_err(Error::from)?;
		Ok(())
	}
}

#[async_trait]
impl GenerationDispatchRepository for NativeDispatch {
	fn node_id(&self) -> &str {
		&self.store.node_id
	}
	async fn begin_preparation(&self) -> Result<Box<dyn DispatchPreparation>> {
		Ok(Box::new(Preparation {
			transaction: self.store.pool.begin().await.map_err(Error::from)?,
		}))
	}
	async fn record(&self, attempt: Uuid) -> Result<Option<Record>> {
		let record: Option<StoredDispatch> = {
			let query_bind_1 = attempt;
			sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("generation_remote_dispatches"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&self.store.pool)
			.await
			.map_err(Error::from)?
		};
		Ok(record.map(Into::into))
	}
	async fn admit(&self, input: &Input, reservations: &[Reserved]) -> Result<u64> {
		let changed = {
			let query_bind_1 = input.usage.attempt_id;
			let query_bind_2 = input.usage.digest()?;
			let query_bind_3 = json!(reservations);
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_remote_dispatches"))
					.value(Alias::new("state"), "DISPATCHED")
					.value_expr(
						Alias::new("reservations"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=? AND digest=? AND state='PREPARING')".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&self.store.pool)
			.await
			.map_err(Error::from)?
		};
		Ok(changed.rows_affected())
	}
	async fn finalize(&self, attempt: Uuid, from: &str, state: &str, value: &Value) -> Result<u64> {
		let changed = {
			let query_bind_1 = attempt;
			let query_bind_2 = from;
			let query_bind_3 = state;
			let query_bind_4 = value;
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_remote_dispatches"))
					.value_expr(
						Alias::new("state"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(
						Alias::new("finalization"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=? AND state=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&self.store.pool)
			.await
			.map_err(Error::from)?
		};
		Ok(changed.rows_affected())
	}
	async fn begin_visibility(&self) -> Result<Box<dyn DispatchVisibility>> {
		Ok(Box::new(Visibility {
			store: self.store.clone(),
			lease: crate::transactions::gate::ReadLease::begin(&self.store).await?,
		}))
	}
	async fn mark_peer_finalized(&self, attempt: Uuid) -> Result<()> {
		{
			let query_bind_1 = attempt;
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_remote_dispatches"))
					.value(Alias::new("peer_finalized"), true)
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&self.store.pool)
			.await
			.map_err(Error::from)?
		};
		Ok(())
	}
}

#[async_trait]
impl DispatchVisibility for Visibility {
	async fn terminal_record(&mut self, attempt: Uuid) -> Result<Option<Record>> {
		let record: Option<StoredDispatch> = {
			let query_bind_1 = attempt;
			sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("generation_remote_dispatches"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=? AND state IN ('ABORTED','SETTLED'))".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&self.store.pool)
			.await
			.map_err(Error::from)?
		};
		Ok(record.map(Into::into))
	}
	async fn abort_stale_preparations(&mut self) -> Result<()> {
		sqlx::query(
			&Query::update()
				.table(Alias::new("generation_remote_dispatches"))
				.value(Alias::new("state"), "ABORTED")
				.value_expr(
					Alias::new("finalization"),
					Expr::cust("'{\"state\":\"aborted\"}'::jsonb"),
				)
				.and_where(Expr::cust(
					"state='PREPARING' AND created_at < CLOCK_TIMESTAMP()-INTERVAL '120 seconds'",
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&self.store.pool)
		.await
		.map_err(Error::from)?;
		Ok(())
	}
	async fn pending(&mut self) -> Result<Vec<Uuid>> {
		let ids: Vec<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("attempt_id"))
				.from(Alias::new("generation_remote_dispatches"))
				.and_where(Expr::cust(
					"state IN ('ABORTED','SETTLED') AND NOT peer_finalized",
				))
				.order_by(Alias::new("created_at"), reinhardt::query::Order::Asc)
				.limit(16)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&self.store.pool)
		.await
		.map_err(Error::from)?;
		Ok(ids)
	}
	async fn suspend(&mut self) -> Result<()> {
		self.lease.suspend().await.map_err(Into::into)
	}
}

#[async_trait]
impl GenerationDispatchSettlement for NativeSettlement {
	async fn local(&self, usage: &Usage, result: &Finalization) -> Result<()> {
		crate::generation::remote::finalize(&self.runtime.store, usage, result)
			.await
			.map_err(Into::into)
	}
	async fn peer(&self, node: &str, input: &FinalizeInput) -> Result<bool> {
		crate::authorization::peer::authority_request(
			&self.runtime,
			node,
			"/scoped/usage/finalize",
			&json!(input),
		)
		.await
		.map_err(Into::into)
	}
}
