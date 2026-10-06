//! Foreign lifecycle queries retain database-time retry CAS and original visibility boundaries.
use crate::{federation::Federation, transactions::gate::ReadLease};
use aidash_application::{
	Result,
	ports::generation::{
		foreign::maintenance::{ForeignGenerationMaintenance, ForeignGenerationVisibility},
		provisioning::{GenerationProvisioning, GenerationTerminalSession},
	},
};
use aidash_domain::{RunPhase, generation::requests::Request};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct NativeMaintenance {
	pub federation: Federation,
}
struct Visibility(ReadLease);
#[async_trait]
impl ForeignGenerationVisibility for Visibility {
	async fn suspend(&mut self) -> Result<()> {
		self.0.suspend().await.map_err(Into::into)
	}
}
#[async_trait]
impl ForeignGenerationMaintenance for NativeMaintenance {
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn begin_visibility(&self) -> Result<Box<dyn ForeignGenerationVisibility>> {
		Ok(Box::new(Visibility(
			ReadLease::begin(&self.federation.store).await?,
		)))
	}
	async fn reserve_retry(&self, id: Uuid) -> Result<u64> {
		let f = &self.federation;
		let reserved = { let query_bind_1 = id; crate::database::native::query(&Query::update()
			.table(Alias::new("generation_remote_intents")).value_expr(Alias::new("cancel_retry_at"), Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 seconds'"))
			.and_where(SimpleExpr::CustomWithExpr("(id=? AND cancelled AND NOT cancel_delivered AND (cancel_retry_at IS NULL OR cancel_retry_at<=CLOCK_TIMESTAMP()))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()]))
			.to_string(PostgresQueryBuilder))
	.execute(&f.store.pool)
	.await? }
	.rows_affected();
		Ok(reserved)
	}
	async fn send_cancel(&self, target: &str, id: Uuid) -> Result<bool> {
		crate::authorization::peer::authority_request(
			&self.federation,
			target,
			"/scoped/generation/cancel",
			&json!({"intent_id":id}),
		)
		.await
		.map_err(Into::into)
	}
	async fn mark_delivered(&self, id: Uuid) -> Result<()> {
		let f = &self.federation;

		{
			let query_bind_1 = id;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("generation_remote_intents"))
					.value(Alias::new("cancel_delivered"), true)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND cancelled)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&f.store.pool)
			.await?
		};
		Ok(())
	}
	async fn pending(&self) -> Result<Vec<(Uuid, Value)>> {
		let f = &self.federation;
		let pending: Vec<(Uuid, Value)> = crate::database::native::query_as(
		&Query::select()
			.columns([Alias::new("id"), Alias::new("binding")])
			.from(Alias::new("generation_remote_intents"))
			.and_where(Expr::cust("cancelled AND NOT cancel_delivered AND (cancel_retry_at IS NULL OR cancel_retry_at<=CLOCK_TIMESTAMP())"))
			.order_by(Alias::new("cancel_retry_at"), reinhardt::query::Order::Asc)
			.limit(16)
			.to_string(PostgresQueryBuilder),
	).columns(&["id", "binding"])
	.fetch_all(&f.store.pool)
	.await?;
		Ok(pending)
	}
	async fn jobs(&self) -> Result<Vec<Request>> {
		let f = &self.federation;
		let jobs:Vec<Request>=crate::database::query_as(&Query::select().column(reinhardt::query::ColumnRef::table_asterisk("g")).from_as(Alias::new("generation_requests"),Alias::new("g")).join(reinhardt::query::JoinType::LeftJoin, reinhardt::query::TableRef::table_alias(Alias::new("runs"), Alias::new("r")), Expr::cust("r.id=g.admission_id AND r.home_node=g.home_node"))
    .and_where(Expr::cust("g.home_node<>'' AND g.status IN ('PENDING_APPROVAL','QUEUED','ACTIVE') AND (g.expires_at<=CLOCK_TIMESTAMP() OR r.phase IN ('COMPLETED','FAILED','CANCELLED'))"))
    .order_by((Alias::new("g"),Alias::new("created_at")),reinhardt::query::Order::Asc).limit(16).to_string(PostgresQueryBuilder)).fetch_all(&f.store.pool).await?;
		Ok(jobs)
	}
	async fn cancel_job(&self, source: &str, id: Uuid) -> Result<Option<Request>> {
		let f = &self.federation;
		let job: Option<Request> = {
			let query_bind_1 = source;
			let query_bind_2 = id.to_string();
			crate::database::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("generation_requests"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(home_node=? AND foreign_intent->>'id'=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&f.store.pool)
			.await?
		};
		Ok(job)
	}
	async fn run_phase(&self, id: Uuid) -> Result<RunPhase> {
		Ok(self.federation.store.run(id).await?.phase())
	}
	async fn begin_terminal(&self, job: &Request) -> Result<Box<dyn GenerationTerminalSession>> {
		crate::bootstrap::generation_provisioning_repository(&self.federation)
			.begin_terminal(job)
			.await
	}
	fn warn_cancel(&self, id: Uuid, error: &aidash_application::Error, retry: bool) {
		if retry {
			tracing::warn!(%error,%id,"remote generation cancellation retry failed");
		} else {
			tracing::warn!(%error,%id,"remote generation cancellation queued for retry");
		}
	}
	fn notify(&self) {
		self.federation.notify.notify_waiters();
	}
}
