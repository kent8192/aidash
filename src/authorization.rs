use sea_orm::sea_query::{
	Alias, CommonTableExpression, Condition, Expr, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, UnionType,
};
pub(crate) mod access;
pub mod api;
pub mod catalog;
pub mod execution;
pub mod identity;
pub mod interaction;
pub mod peer;
pub mod policy;
pub mod remote;
mod resources;
pub mod workspace;

use crate::{Error, Result};
use policy::{Decision, Evaluation, PolicyBundle, identifier};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct Authorization {
	pub pool: PgPool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Snapshot {
	pub revision: i64,
	pub bundle: PolicyBundle,
}

impl Authorization {
	pub async fn replace(
		&self,
		tenant: &str,
		expected_revision: i64,
		bundle: PolicyBundle,
		actor: &str,
	) -> Result<Snapshot> {
		bundle.validate()?;
		identifier(actor)?;
		if bundle.tenant != tenant || expected_revision < 0 || expected_revision == i64::MAX {
			return Err(Error::Invalid(
				"tenant mismatch or invalid expected revision".into(),
			));
		}
		let mut tx = self.pool.begin().await?;
		crate::transactions::authority::control(&mut tx).await?;
		let document = serde_json::to_value(&bundle)?;
		let revision: Option<i64> = if expected_revision == 0 {
			sqlx::query_scalar(
				&Query::insert()
					.into_table(Alias::new("authorization_bundles"))
					.columns([
						Alias::new("tenant"),
						Alias::new("revision"),
						Alias::new("document"),
					])
					.values_panic([Expr::cust("$1"), Expr::cust("1"), Expr::cust("$2")])
					.on_conflict(
						OnConflict::columns([Alias::new("tenant")])
							.do_nothing()
							.to_owned(),
					)
					.returning(Query::returning().columns([Alias::new("revision")]))
					.to_string(PostgresQueryBuilder),
			)
			.bind(tenant)
			.bind(&document)
			.fetch_optional(&mut *tx)
			.await?
		} else {
			sqlx::query_scalar(
				&Query::update()
					.table(Alias::new("authorization_bundles"))
					.value(Alias::new("revision"), Expr::cust("revision+1"))
					.value(Alias::new("document"), Expr::cust("$3"))
					.value(Alias::new("updated_at"), Expr::cust("now()"))
					.cond_where(
						Condition::all()
							.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
							.add(Expr::col(Alias::new("revision")).eq(Expr::cust("$2"))),
					)
					.returning(Query::returning().columns([Alias::new("revision")]))
					.to_string(PostgresQueryBuilder),
			)
			.bind(tenant)
			.bind(expected_revision)
			.bind(&document)
			.fetch_optional(&mut *tx)
			.await?
		};
		let revision =
			revision.ok_or_else(|| Error::Conflict("authorization revision changed".into()))?;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("authorization_revisions"))
				.columns([
					Alias::new("tenant"),
					Alias::new("revision"),
					Alias::new("document"),
					Alias::new("actor"),
				])
				.values_panic([
					Expr::cust("$1"),
					Expr::cust("$2"),
					Expr::cust("$3"),
					Expr::cust("$4"),
				])
				.to_string(PostgresQueryBuilder),
		)
		.bind(tenant)
		.bind(revision)
		.bind(&document)
		.bind(actor)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(Snapshot { revision, bundle })
	}

	pub(crate) async fn load(tx: &mut Transaction<'_, Postgres>, tenant: &str) -> Result<Snapshot> {
		Self::load_with_mode(tx, tenant, false).await
	}

	pub(crate) async fn load_with_mode(
		tx: &mut Transaction<'_, Postgres>,
		tenant: &str,
		exclusive: bool,
	) -> Result<Snapshot> {
		identifier(tenant)?;
		let query = if exclusive {
			Query::select()
				.column(Alias::new("revision"))
				.column(Alias::new("document"))
				.from(Alias::new("authorization_bundles"))
				.cond_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder)
		} else {
			Query::select()
				.column(Alias::new("revision"))
				.column(Alias::new("document"))
				.from(Alias::new("authorization_bundles"))
				.cond_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder)
		};
		let row: Option<(i64, Value)> = sqlx::query_as(&query)
			.bind(tenant)
			.fetch_optional(&mut **tx)
			.await?;
		let (revision, document) =
			row.ok_or_else(|| Error::NotFound("authorization policy".into()))?;
		Ok(Snapshot {
			revision,
			bundle: serde_json::from_value(document)?,
		})
	}

	pub async fn snapshot(&self, tenant: &str) -> Result<Snapshot> {
		let mut tx = self.pool.begin().await?;
		let snapshot = Self::load(&mut tx, tenant).await?;
		tx.commit().await?;
		Ok(snapshot)
	}

	/// Check and audit under a shared policy lock. Mutation callers must use this
	/// same transaction so revocation cannot race their authorization decision.
	pub async fn evaluate_in_transaction(
		tx: &mut Transaction<'_, Postgres>,
		tenant: &str,
		input: &Evaluation,
	) -> Result<Decision> {
		input.validate()?;
		let snapshot = Self::load(tx, tenant).await?;
		let mut decision = snapshot.bundle.evaluate(input);
		decision.revision = snapshot.revision;
		Self::record(tx, tenant, input, &decision).await?;
		Ok(decision)
	}

	pub(super) async fn record(
		tx: &mut Transaction<'_, Postgres>,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		Self::record_many(tx, tenant, &[(input.clone(), decision.clone())]).await
	}

	/// Retain every decision and its allocation/commit order, while acquiring
	/// the transaction-wide audit controls only once for a compound check.
	pub(super) async fn record_many(
		tx: &mut Transaction<'_, Postgres>,
		tenant: &str,
		records: &[(Evaluation, Decision)],
	) -> Result<()> {
		if records.is_empty() {
			return Ok(());
		}
		crate::transactions::authority::control(tx).await?;
		// Bound parameter count even for large replay/authorization operations.
		for records in records.chunks(100) {
			let columns = [
				"tenant",
				"revision",
				"subject",
				"action",
				"resource_kind",
				"resource_id",
				"decision",
			];
			let mut values = Query::select();
			for index in 0..records.len() {
				let mut row = Query::select();
				for (column, name) in columns.iter().enumerate() {
					let ty = match column {
						1 => "bigint",
						6 => "jsonb",
						_ => "text",
					};
					row.expr_as(
						Expr::cust(format!("${}::{ty}", index * 7 + column + 1)),
						Alias::new(*name),
					);
				}
				row.expr_as(Expr::val(index as i32), Alias::new("ordinal"));
				if index == 0 {
					values = row;
				} else {
					values.union(UnionType::All, row);
				}
			}
			// The INSERT's source must read this materialized barrier before
			// defaults allocate any audit sequence. Retain the lock until commit,
			// but avoid a client round trip between acquiring it and inserting.
			let barrier = CommonTableExpression::new()
				.table_name("audit_lock")
				.materialized(true)
				.query(
					Query::select()
						.expr(Expr::cust("pg_advisory_xact_lock(71003202)"))
						.to_owned(),
				)
				.to_owned();
			let mut insert = Query::insert();
			insert
				.with_cte(barrier)
				.into_table(Alias::new("authorization_decisions"))
				.columns(columns.map(Alias::new))
				.select_from(
					Query::select()
						.columns(columns.map(|name| (Alias::new("decisions"), Alias::new(name))))
						.from(Alias::new("audit_lock"))
						.from_subquery(values, Alias::new("decisions"))
						.order_by((Alias::new("decisions"), Alias::new("ordinal")), Order::Asc)
						.to_owned(),
				)
				.expect("audit source matches insert columns");
			let sql = insert.to_string(PostgresQueryBuilder);
			let mut query = sqlx::query(&sql);
			for (input, decision) in records {
				query = query
					.bind(tenant)
					.bind(decision.revision)
					.bind(&input.subject)
					.bind(&input.action)
					.bind(&input.resource.kind)
					.bind(&input.resource.id)
					.bind(serde_json::to_value(decision)?);
			}
			query.execute(&mut **tx).await?;
		}
		Ok(())
	}

	pub async fn evaluate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		let mut tx = self.pool.begin().await?;
		let decision = Self::evaluate_in_transaction(&mut tx, tenant, input).await?;
		tx.commit().await?;
		Ok(decision)
	}

	pub async fn simulate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		input.validate()?;
		let snapshot = self.snapshot(tenant).await?;
		let mut decision = snapshot.bundle.evaluate(input);
		decision.revision = snapshot.revision;
		Ok(decision)
	}

	pub async fn revisions(&self, tenant: &str, after: i64, limit: i64) -> Result<Vec<Value>> {
		identifier(tenant)?;
		Ok(sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("to_jsonb(r)"))
				.from_as(Alias::new("authorization_revisions"), Alias::new("r"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("revision")).gt(Expr::cust("$2"))),
				)
				.order_by(Alias::new("revision"), Order::Asc)
				.limit(limit.clamp(1, 200) as u64)
				.to_string(PostgresQueryBuilder),
		)
		.bind(tenant)
		.bind(after)
		.fetch_all(&self.pool)
		.await?)
	}

	pub async fn decisions(&self, tenant: &str, after: i64, limit: i64) -> Result<Vec<Value>> {
		identifier(tenant)?;
		Ok(sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("to_jsonb(d)"))
				.from_as(Alias::new("authorization_decisions"), Alias::new("d"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("sequence")).gt(Expr::cust("$2"))),
				)
				.order_by(Alias::new("sequence"), Order::Asc)
				.limit(limit.clamp(1, 200) as u64)
				.to_string(PostgresQueryBuilder),
		)
		.bind(tenant)
		.bind(after)
		.fetch_all(&self.pool)
		.await?)
	}
}
