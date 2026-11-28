//! Native tombstones and thread locks retain the original ordered query expressions.
use crate::apps::execution::capabilities::serializers::thread_lifecycle::DeleteThread as NativeDeleteThread;
use crate::{Error as NativeError, Result as NativeResult};
use crate::{
	apps::execution::capabilities::services::{records, sessions},
	authorization::access::Access,
	store::Store,
};
use aidash_application::{Result, ports::capabilities::thread_lifecycle::ThreadScope};
use aidash_domain::capabilities::thread_lifecycle::DeleteThread;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) store: &'a Store,
	pub(crate) access: &'a mut Access,
}

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
	fn from(value: NativeDeleteThread) -> Self {
		Self {
			idempotency_key: value.idempotency_key,
		}
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
	fn principal(&self) -> &str {
		&self.access.identity.subject
	}
	async fn serialize(&mut self) -> Result<()> {
		super::sharing::serialize(self.access)
			.await
			.map_err(Into::into)
	}
	async fn thread(&mut self, id: Uuid) -> Result<aidash_domain::policy::Resource> {
		self.access.thread(id).await.map_err(Into::into)
	}
	async fn require(
		&mut self,
		resource: &aidash_domain::policy::Resource,
		action: &str,
	) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn visible_thread(&mut self, thread: Uuid) -> Result<()> {
		visible(&mut self.access.tx, thread)
			.await
			.map_err(Into::into)
	}
	async fn record_tombstone(&mut self, thread: Uuid) -> Result<()> {
		records::insert(
			self.access,
			thread,
			None,
			"thread_tombstone",
			"deleted",
			serde_json::json!({"thread_id":thread}),
			None,
		)
		.await?;
		Ok(())
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		sessions::cached(self.access, key, digest)
			.await
			.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()> {
		sessions::cache(self.access, key, digest, result)
			.await
			.map_err(Into::into)
	}
	async fn lock_for_deletion(&mut self, thread: Uuid) -> Result<()> {
		crate::apps::workspaces::models::Thread::lock_for_deletion(&mut *self.access.tx, thread)
			.await
			.map_err(Into::into)
	}
	async fn mark_deleted(&mut self, thread: Uuid) -> Result<()> {
		let query = Query::update()
			.table(Alias::new("threads"))
			.value(Alias::new("scratch_wipe_requested"), true)
			.value_expr(Alias::new("deleted_at"), Expr::cust("CURRENT_TIMESTAMP"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(thread)))
			.and_where(Expr::col(Alias::new("deleted_at")).is_null())
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&query)
			.execute(&mut **self.access.tx)
			.await?;
		Ok(())
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
	async fn stop_work(&mut self, thread: Uuid) -> Result<()> {
		let store = self.store;
		let result: NativeResult<()> = async {
			let terminal = ["COMPLETED", "FAILED", "CANCELLED"];
			let runs: Vec<Uuid> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("task_executions"))
					.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(thread)))
					.and_where(Expr::col(Alias::new("phase")).is_not_in(terminal))
					.order_by(Alias::new("id"), Order::Asc)
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_all(&mut **self.access.tx)
			.await?;
			// Lock every end before the first control event takes the event
			// lock, as a Node's cancellation does.
			for run in &runs {
				super::subnodes::lock_for_end(&mut **self.access.tx, *run).await?;
			}
			for run in runs {
				// A creator's cascade may
				// already have cancelled the Run.
				let open: bool = crate::database::native::query_scalar(
					&Query::select()
						.expr(Expr::exists(
							Query::select()
								.expr(Expr::value(1_i32))
								.from(Alias::new("task_executions"))
								.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run)))
								.and_where(Expr::col(Alias::new("phase")).is_not_in(terminal))
								.and_where(
									Expr::col(Alias::new("control")).ne(Expr::value("CANCELLED")),
								)
								.to_owned(),
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **self.access.tx)
				.await?;
				if open {
					store
						.control_in_as(
							&mut **self.access.tx,
							run,
							aidash_domain::RunControlAction::Cancel,
							aidash_domain::ControlSource::Tenant,
						)
						.await?;
				}
			}
			let tasks: Vec<(Uuid, i64, Option<String>)> = crate::database::native::query_as(
				&Query::select()
					.columns(["id", "revision", "owner"].map(Alias::new))
					.from(Alias::new("tasks"))
					.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(thread)))
					.and_where(Expr::col(Alias::new("status")).is_not_in([
						"COMPLETED",
						"FAILED",
						"CANCELLED",
						"ABANDONED",
					]))
					.order_by(Alias::new("id"), Order::Asc)
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["id", "revision", "owner"])
			.fetch_all(&mut **self.access.tx)
			.await?;
			for (task, revision, owner) in tasks {
				store
					.transition_in(
						&mut self.access.tx,
						task,
						revision,
						owner.as_deref().unwrap_or("thread-deletion"),
						aidash_domain::TaskStatus::Cancelled,
					)
					.await?;
			}
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn event(&mut self, thread: Uuid, kind: &str, data: Value) -> Result<()> {
		self.store
			.event(&mut self.access.tx, Some(thread), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
