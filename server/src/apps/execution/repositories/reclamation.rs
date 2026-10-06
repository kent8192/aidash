//! Native maintenance transactions retain every object and quota ownership predicate.
use super::capability_records::{domain, native};
use crate::apps::execution::capabilities::services::{
	python,
	records::{self, Record as NativeRecord},
	sessions,
};
use crate::{Result as NativeResult, store::Store};
use aidash_application::{
	Result,
	ports::capabilities::reclamation::{ReclamationRepository, ReclamationScope},
};
use aidash_domain::capabilities::records::Record;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	store: &'a Store,
	orphan_cursor: tokio::sync::Mutex<Option<tokio::fs::ReadDir>>,
}
impl<'a> Repository<'a> {
	pub(crate) fn new(store: &'a Store) -> Self {
		Self {
			store,
			orphan_cursor: tokio::sync::Mutex::new(None),
		}
	}
}
struct Scope<'a> {
	store: &'a Store,
	tx: crate::database::native::Transaction,
}
#[async_trait]
impl ReclamationRepository for Repository<'_> {
	async fn begin(&self) -> Result<Box<dyn ReclamationScope + '_>> {
		Ok(Box::new(Scope {
			store: self.store,
			tx: crate::database::native::begin(&self.store.pool).await?,
		}))
	}
	async fn retained(&self, after: Uuid) -> Result<Vec<Uuid>> {
		let result:NativeResult<Vec<Uuid>>=async {
let ids: Vec<Uuid> = {
			let query_bind_1 = after;
			crate::database::native::query_scalar(&Query::select().column(Alias::new("id")).from(Alias::new("core_records"))
            .and_where(Expr::col(Alias::new("kind")).is_in(["transfer_in", "transfer_out", "reference"]))
            .and_where(Expr::cust("COALESCE(data->>'objects_released','false') <> 'true'"))
            .and_where(Condition::any()
                .add(Expr::cust("kind = 'transfer_out' AND (state IN ('delivered','blocked') OR expires_at <= CURRENT_TIMESTAMP)"))
                .add(Expr::cust("kind = 'transfer_in' AND (state = 'committed' OR expires_at <= CURRENT_TIMESTAMP)"))
                .add(Expr::cust("kind = 'reference' AND (state = 'revoked' OR expires_at <= CURRENT_TIMESTAMP OR (state IN ('ready','extracting') AND jsonb_array_length(data->'chunks') > 0))")))
            .and_where(Expr::col(Alias::new("id")).gt(Expr::value(query_bind_1.to_owned())))
            .order_by(Alias::new("id"), reinhardt::query::Order::Asc)
            .limit(8).to_string(PostgresQueryBuilder)).scalar_all(&self.store.pool).await?
		};
        Ok(ids)
    }.await;
		result.map_err(Into::into)
	}
	async fn working(&self, after: Uuid) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			let ids: Vec<Uuid> = {
				let query_bind_1 = after;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("core_objects"))
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("superseded_working")),
						)
						.and_where(
							Expr::col(Alias::new("id")).gt(Expr::value(query_bind_1.to_owned())),
						)
						.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
						.limit(16)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_all(&self.store.pool)
				.await?
			};
			Ok(ids)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn python(&self, cursor: &mut Uuid) -> Result<()> {
		python::reap(self.store, cursor).await.map_err(Into::into)
	}
	async fn orphans(&self) -> Result<()> {
		let mut cursor = self.orphan_cursor.lock().await;
		self.store
			.capabilities
			.reconcile_orphan_batch(&self.store.pool, &mut cursor)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl ReclamationScope for Scope<'_> {
	async fn load(&mut self, id: Uuid) -> Result<Record> {
		let result: NativeResult<NativeRecord> = async {
			let record: NativeRecord = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&sessions::select("core_records")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut *self.tx)
				.await?
			};
			Ok(record)
		}
		.await;
		result.map(domain).map_err(Into::into)
	}
	async fn release_quota(&mut self, tenant: &str, reserved: i64) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = tenant;
				let query_bind_2 = reserved;
				crate::database::native::query(
					&Query::update()
						.table(Alias::new("core_quotas"))
						.value_expr(
							Alias::new("used_bytes"),
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("used_bytes")))
								.sub(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("used_bytes")))
								.gte(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *self.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn object_kind(&mut self, id: Uuid, tenant: &str) -> Result<Option<String>> {
		let result: NativeResult<Option<String>> = async {
			let kind: Option<String> = {
				let query_bind_1 = id;
				let query_bind_2 = tenant;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("kind"))
						.from(Alias::new("core_objects"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								),
							),
						)
						.and_where(Expr::col(Alias::new("area_id")).is_null())
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut *self.tx)
				.await?
			};
			Ok(kind)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn working_tenant(&mut self, id: Uuid) -> Result<Option<String>> {
		let result: NativeResult<Option<String>> = async {
			let tenant: Option<String> = {
				let query_bind_1 = id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("tenant"))
						.from(Alias::new("core_objects"))
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("superseded_working")),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut *self.tx)
				.await?
			};
			Ok(tenant)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn erase(&mut self, tenant: &str, id: Uuid) -> Result<()> {
		self.store
			.capabilities
			.erase_committed(&mut self.tx, tenant, id)
			.await
			.map_err(Into::into)
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut current = native(record);
		records::update_committed(&mut self.tx, &mut current).await?;
		record.revision = current.revision;
		Ok(())
	}
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		use aidash_application::ports::capabilities::runner::RunnerTransport as _;
		crate::bootstrap::operation_runner(self.store)?
			.request(method, path, body.as_ref())
			.await
	}
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<()> {
		let Scope { tx, .. } = *self;
		match result {
			Ok(true) => tx.commit().await.map_err(|error| error.into()),
			Ok(false) => Ok(()),
			Err(error) => Err(error),
		}
	}
}
