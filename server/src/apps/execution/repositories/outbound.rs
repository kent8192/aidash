//! Native authority transactions and unchanged PostgreSQL statements implement outbound ports.
use super::capability_records::{domain, native};
use crate::apps::execution::capabilities::services::{
	approvals,
	records::{self, Record as NativeRecord},
	sessions,
};
use crate::{
	Result as NativeResult,
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::outbound::{
		AuthorizedRun, FetchPolicy, OutboundRepository, OutboundScope,
	},
};
use aidash_domain::capabilities::records::Record;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) store: &'a Store,
}
struct Scope<'a> {
	store: &'a Store,
	access: Access,
}

#[async_trait]
impl OutboundRepository for Repository<'_> {
	fn fetch_policy(&self) -> FetchPolicy {
		FetchPolicy {
			origins: self.store.capabilities.0.outbound_origins.clone(),
			output_bytes: self.store.capabilities.0.output_bytes,
		}
	}
	async fn active_operations(&self) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			let ids: Vec<Uuid> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_records"))
					.and_where(
						Expr::col(Alias::new("kind")).eq(reinhardt::query::Expr::value("outbound")),
					)
					.and_where(
						Condition::any()
							.add(
								Expr::col(Alias::new("state"))
									.eq(reinhardt::query::Expr::value("approved")),
							)
							.add(
								Condition::all()
									.add(
										Expr::col(Alias::new("state"))
											.eq(reinhardt::query::Expr::value("attempted")),
									)
									.add(Expr::cust(
										"(data->>'attempt_deadline')::timestamptz < CURRENT_TIMESTAMP",
									)),
							),
					)
					.order_by(Alias::new("id"), Order::Asc)
					.limit(8)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&self.store.pool)
			.await?;
			Ok(ids)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn snapshot(&self, id: Uuid) -> Result<Record> {
		let result: NativeResult<Record> = async {
			let snapshot: NativeRecord = {
				let query_bind_1 = id;
				sqlx::query_as(
					&sessions::select("core_records")
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
				.fetch_one(&self.store.pool)
				.await?
			};
			Ok(domain(snapshot))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn begin(&self, record: &Record) -> Result<Box<dyn OutboundScope + '_>> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: serde_json::from_value(record.data["credential_id"].clone())?,
			tenant: record.tenant.clone(),
			subject: record.owner.clone(),
		};
		let access = Access::begin(self.store, &identity).await?;
		Ok(Box::new(Scope {
			store: self.store,
			access,
		}))
	}
	async fn fail(&self, id: Uuid, message: &str) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = id;
				let query_bind_2 =
					aidash_domain::capabilities::outbound::failure_disclosure(message);
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_records"))
						.value_expr(Alias::new("state"), Expr::val("uncertain"))
						.value_expr(
							Alias::new("data"),
							SimpleExpr::CustomWithExpr(
								"(data || ?::jsonb)".into(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.value_expr(
							Alias::new("revision"),
							Expr::col(Alias::new("revision")).add(reinhardt::query::Expr::value(1)),
						)
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(Expr::col(Alias::new("state")).is_in(["approved", "attempted"]))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&self.store.pool)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl OutboundScope for Scope<'_> {
	async fn load(&mut self, id: Uuid) -> Result<Record> {
		records::get(&mut self.access, id, "outbound")
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn authorize(&mut self, record: &Record) -> Result<AuthorizedRun> {
		let run = approvals::authorize(self.store, &mut self.access, &native(record)).await?;
		Ok(AuthorizedRun {
			id: run.id,
			workspace_id: run.workspace_id,
		})
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut current = native(record);
		records::update(&mut self.access, &mut current).await?;
		record.revision = current.revision;
		Ok(())
	}
	async fn put(&mut self, area: Option<Uuid>, bytes: &[u8]) -> Result<(Uuid, String)> {
		self.store
			.capabilities
			.put(&mut self.access, area, "network_output", bytes)
			.await
			.map_err(Into::into)
	}
	async fn event(&mut self, run: &AuthorizedRun, id: Uuid, status: u16) -> Result<()> {
		self.store
			.event(
				&mut self.access.tx,
				Some(run.workspace_id),
				"capability.outbound_completed",
				json!({"operation_id":id,"run_id":run.id,"http_status":status}),
			)
			.await
			.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<Option<Record>>) -> Result<Option<Record>> {
		let Scope { access, .. } = *self;
		access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
