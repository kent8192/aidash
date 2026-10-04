//! Native tombstones and thread locks retain the original ordered query expressions.
use super::cleanup::Scope;
use crate::apps::execution::capabilities::{
	serializers::{
		contracts::Area as NativeArea, thread_lifecycle::DeleteThread as NativeDeleteThread,
	},
	services::sessions,
};
use crate::{Error as NativeError, Result as NativeResult};
use aidash_application::{Error, Result, ports::capabilities::thread_lifecycle::ThreadScope};
use aidash_domain::{
	capabilities::{
		cleanup::Choice,
		sessions::Area,
		thread_lifecycle::{DeleteThread, FileChoice},
	},
	workspaces::channels::ChannelThread,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn visible(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	thread: Uuid,
) -> NativeResult<()> {
	let tombstone: Option<Uuid> = {
		let query_bind_1 = thread;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_records"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("kind")))
						.eq(Expr::cust("'thread_tombstone'")),
				)
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
		.fetch_optional(&mut **tx)
		.await?
	};
	if tombstone.is_some() {
		return Err(NativeError::NotFound("thread unavailable".into()));
	}
	Ok(())
}
impl From<NativeDeleteThread> for DeleteThread {
	fn from(v: NativeDeleteThread) -> Self {
		Self {idempotency_key:v.idempotency_key,files:v.files.into_iter().map(|f|FileChoice {area_id:f.area_id,expected_revision:f.expected_revision,confirmation_id:f.confirmation_id,choice:match f.choice {crate::apps::execution::capabilities::serializers::cleanup::Choice::Keep=>Choice::Keep,crate::apps::execution::capabilities::serializers::cleanup::Choice::Recoverable=>Choice::Recoverable,crate::apps::execution::capabilities::serializers::cleanup::Choice::Irreversible=>Choice::Irreversible}}).collect()}
	}
}
#[async_trait]
impl ThreadScope for Scope<'_> {
	async fn channel(&mut self, workspace: Uuid, thread: Uuid) -> Result<Option<ChannelThread>> {
		let result: NativeResult<Option<ChannelThread>> = async {
			let access = &mut *self.access;
			let row: Option<crate::apps::workspaces::models::ChannelThread> = {
				let query_bind_1 = thread;
				let query_bind_2 = workspace;
				crate::database::query_as(
					&sessions::select("channel_threads")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"workspace_id",
							)))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							)),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(row.map(crate::collaboration::ChannelThread::from))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn owned_areas(
		&mut self,
		workspace: Uuid,
		thread: Uuid,
		offset: u64,
	) -> Result<Vec<Area>> {
		let result: NativeResult<Vec<NativeArea>> = async {
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = workspace;
				let query_bind_2 = thread;
				let query_bind_3 = &access.identity.subject;
				sqlx::query_as(
					&sessions::select("core_areas")
				.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::value(query_bind_1.to_owned())))
				.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(query_bind_2.to_owned())))
				// Deleting a shared conversation never grants management of another
				// subject's private files. Their areas remain available in settings.
				.and_where(Expr::col(Alias::new("owner")).eq(Expr::value(query_bind_3.to_owned())))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(100)
				.offset(offset)
				.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **access.tx)
				.await?
			})
		}
		.await;
		result
			.map(|areas| areas.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn cancel_generation(&mut self, area: &Area) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = area.id;
				sqlx::query(
					&Query::update()
						.table(Alias::new("runs"))
						.value(Alias::new("control"), "CANCELLED")
						.and_where(
							Expr::col(Alias::new("id")).in_subquery(
								Query::select()
									.column(Alias::new("run_id"))
									.from(Alias::new("core_runs"))
									.and_where(
										Expr::col(Alias::new("area_id"))
											.eq(Expr::value(query_bind_1.to_owned())),
									)
									.to_owned(),
							),
						)
						.and_where(Expr::col(Alias::new("phase")).is_not_in([
							"COMPLETED",
							"FAILED",
							"CANCELLED",
						]))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.store
			.ok_or_else(|| Error::External("thread repository scope invariant".into()))?
			.event(&mut self.access.tx, Some(workspace), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
