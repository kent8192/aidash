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
use aidash_domain::capabilities::{
	cleanup::Choice,
	sessions::Area,
	thread_lifecycle::{DeleteThread, FileChoice},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn visible(
	tx: &mut crate::database::native::Transaction,
	thread: Uuid,
) -> NativeResult<()> {
	let tombstone: Option<Uuid> = {
		let query_bind_1 = thread;
		crate::database::native::query_scalar(
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
		.scalar_optional(&mut **tx)
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
impl aidash_application::ports::capabilities::thread_lifecycle::DeletionAuthority for Scope<'_> {
	async fn decide(
		&mut self,
		resource: &aidash_domain::policy::Resource,
		action: &str,
	) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn node_resource(&mut self, thread: Uuid) -> Result<aidash_domain::policy::Resource> {
		let node = self.access.thread_node(thread).await?;
		Ok(self
			.access
			.resource("node", node.as_str(), serde_json::json!({})))
	}
}
#[async_trait]
impl ThreadScope for Scope<'_> {
	async fn lock_for_deletion(&mut self, thread: Uuid) -> Result<()> {
		crate::apps::workspaces::models::Thread::lock_existing(&mut *self.access.tx, thread)
			.await
			.map_err(Into::into)
	}
	async fn mark_deleted(&mut self, thread: Uuid) -> Result<()> {
		let query = Query::update()
			.table(Alias::new("threads"))
			.value_expr(Alias::new("deleted_at"), Expr::cust("CURRENT_TIMESTAMP"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(thread)))
			.and_where(Expr::col(Alias::new("deleted_at")).is_null())
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&query)
			.execute(&mut **self.access.tx)
			.await?;
		Ok(())
	}
	async fn owned_areas(&mut self, thread: Uuid, offset: u64) -> Result<Vec<Area>> {
		let result: NativeResult<Vec<NativeArea>> = async {
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = thread;
				let query_bind_3 = &access.identity.subject;
				crate::database::native::query_as(
					&sessions::select("core_areas")
				.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(query_bind_1.to_owned())))
				// Deleting a shared Thread never grants management of another
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
	async fn delete_attachments(&mut self, thread: Uuid) -> Result<()> {
		// Indexed images leave the Thread index with their attachments
		// (#622 decision 11); the cleanup sweep removes their vectors.
		let images = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("channel_attachments"))
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(thread)))
			.and_where(Expr::col(Alias::new("media_type")).like("image/%"))
			.to_string(PostgresQueryBuilder);
		let images: Vec<Uuid> = crate::database::native::query_scalar(&images)
			.scalar_all(&mut **self.access.tx)
			.await?;
		{
			let mut lease =
				crate::apps::knowledge::repositories::access::Lease::Inherited(&mut *self.access);
			for image in images {
				crate::apps::knowledge::repositories::attachment_images::retire(
					&mut lease, thread, image,
				)
				.await?;
			}
		}
		let query = Query::delete()
			.from_table(Alias::new("channel_attachments"))
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(thread)))
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&query)
			.execute(&mut **self.access.tx)
			.await?;
		Ok(())
	}
	async fn stop_schedules(&mut self, thread: Uuid) -> Result<()> {
		let tenant = self.access.identity.tenant.clone();
		crate::apps::workspaces::services::duties::stop_for_thread(
			&mut **self.access.tx,
			&tenant,
			thread,
		)
		.await?;
		Ok(())
	}
	async fn cancel_generation(&mut self, area: &Area) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = area.id;
				crate::database::native::query(
					&Query::update()
						.table(Alias::new("task_executions"))
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
	async fn event(&mut self, thread: Uuid, kind: &str, data: Value) -> Result<()> {
		self.store
			.ok_or_else(|| Error::External("thread repository scope invariant".into()))?
			.event(&mut self.access.tx, Some(thread), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
