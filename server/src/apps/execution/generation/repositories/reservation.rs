//! Current policy/catalog locks and owned budget transactions implement usage ports.
use super::settlement::{NativeSettlementScope, counter};
use crate::{Error, store::Store};
use aidash_application::{
	Result,
	ports::{
		catalog::CatalogScope,
		generation::{
			reservation::{
				GenerationReservationRepository, GenerationReservationSession,
				GenerationUsageAuthority,
			},
			settlement::GenerationSettlementScope,
		},
	},
};
use aidash_domain::{
	generation::{
		remote::{Attempt, ReservationBinding, Usage},
		requests::Request,
	},
	policy::Resource,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::ColumnRef::Asterisk;
use reinhardt::query::{
	Alias, Expr, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(crate) struct NativeUsageAuthority<'a> {
	pub catalog: crate::apps::identity::repositories::catalog::NativeCatalog<'a>,
}
#[async_trait]
impl GenerationUsageAuthority for NativeUsageAuthority<'_> {
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn jobs(&mut self, node: &str) -> Result<Vec<Request>> {
		let access = &mut *self.catalog.0;

		Ok({
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = node;
			let query_bind_3 = &access.subjects;
			crate::database::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("generation_requests"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(tenant=? AND (? || '/agents/' || agent_id || '@' || agent_version)=ANY(?))"
						.to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						crate::database::text_array(query_bind_3.to_owned()),
					],
				))
				.order_by(Alias::new("id"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&mut **access.tx)
		.await.map_err(Error::from)?
		})
	}
	async fn policy_enabled(&mut self, job: &Request) -> Result<bool> {
		let policy =
			super::policy::load(&mut self.catalog.0.tx, &job.tenant, &job.policy_id, false).await?;
		Ok(policy.spec.enabled)
	}
	async fn pinned_policy(&mut self, job: &Request) -> Result<Value> {
		let access = &mut *self.catalog.0;
		let value: Value = {
			let query_bind_1 = &job.tenant;
			let query_bind_2 = &job.policy_id;
			let query_bind_3 = job.policy_revision;
			sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("spec"))
					.from(Alias::new("generation_policy_history"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant=? AND policy_id=? AND revision=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **access.tx)
			.await
			.map_err(Error::from)?
		};
		Ok(value)
	}
	fn catalog(&mut self) -> &mut dyn CatalogScope {
		&mut self.catalog
	}
	fn remote_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.catalog.0.resource(kind, id, attributes)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.catalog
			.0
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
}

pub(crate) struct NativeReservation {
	pub store: Store,
}
struct Session {
	transaction: Transaction<'static, Postgres>,
}
#[async_trait]
impl GenerationReservationRepository for NativeReservation {
	fn node_id(&self) -> &str {
		&self.store.node_id
	}
	async fn begin(&self) -> Result<Box<dyn GenerationReservationSession>> {
		Ok(Box::new(Session {
			transaction: self.store.pool.begin().await.map_err(Error::from)?,
		}))
	}
}
#[async_trait]
impl GenerationReservationSession for Session {
	async fn lock_attempt(&mut self, attempt: Uuid, digest: &str) -> Result<Attempt> {
		NativeSettlementScope {
			transaction: &mut self.transaction,
		}
		.lock_attempt(attempt, digest)
		.await
	}
	async fn lock_budget(&mut self, job: &Request) -> Result<()> {
		let tx = &mut self.transaction;
		let _: Uuid = {
			let query_bind_1 = job.id;
			sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("request_id"))
					.from(Alias::new("generation_budgets"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(request_id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await
			.map_err(Error::from)?
		};
		Ok(())
	}
	async fn existing(
		&mut self,
		job: &Request,
		usage: &Usage,
	) -> Result<Option<ReservationBinding>> {
		let tx = &mut self.transaction;
		let existing: Option<(String, String)> = {
			let query_bind_1 = job.id;
			let query_bind_2 = usage.attempt_id;
			sqlx::query_as(
				&Query::select()
					.columns(["digest", "state"].map(Alias::new))
					.from(Alias::new("generation_remote_usage"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(request_id=? AND attempt_id=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await
			.map_err(Error::from)?
		};
		Ok(existing.map(|(digest, state)| ReservationBinding { digest, state }))
	}
	async fn debit_budget(&mut self, job: &Request, usage: &Usage) -> Result<u64> {
		let tx = &mut self.transaction;
		let mut q = Query::update();
		q.table(Alias::new("generation_budgets"))
			.value_expr(Alias::new("used_tokens"), Expr::cust("used_tokens+$2"))
			.and_where(Expr::cust(
				"request_id=$1 AND token_limit-used_tokens >= $2",
			));
		if let Some((counter, limit)) = counter(usage.purpose) {
			q.value_expr(Alias::new(counter), Expr::cust(format!("{counter}+1")))
				.and_where(Expr::cust(format!("{counter} < {limit}")));
		}
		let changed = sqlx::query(&q.to_string(PostgresQueryBuilder))
			.bind(job.id)
			.bind(usage.reserved_tokens)
			.execute(&mut **tx)
			.await
			.map_err(Error::from)?;
		Ok(changed.rows_affected())
	}
	async fn insert(&mut self, job: &Request, usage: &Usage, digest: &str) -> Result<()> {
		let tx = &mut self.transaction;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("generation_remote_usage"))
				.columns(
					[
						"request_id",
						"attempt_id",
						"operation_id",
						"dispatcher_node",
						"grant_id",
						"admission_id",
						"purpose",
						"digest",
						"reserved_tokens",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::cust("$1"))
						.expr(Expr::cust("$2"))
						.expr(Expr::cust("$3"))
						.expr(Expr::cust("$4"))
						.expr(Expr::cust("$5"))
						.expr(Expr::cust("$6"))
						.expr(Expr::cust("$7"))
						.expr(Expr::cust("$8"))
						.expr(Expr::cust("$9"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(job.id)
		.bind(usage.attempt_id)
		.bind(usage.operation_id)
		.bind(&usage.dispatcher_node)
		.bind(usage.grant_id)
		.bind(usage.admission_id)
		.bind(usage.purpose.name())
		.bind(digest)
		.bind(usage.reserved_tokens)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.transaction.commit().await.map_err(Error::from)?;
		Ok(())
	}
}
