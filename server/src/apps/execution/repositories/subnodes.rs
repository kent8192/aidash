//! Transactional Subnode lifecycle effects.
use crate::{Result as NativeResult, store::Store};
use aidash_application::{Result, ports::subnodes::EndScope};
use aidash_domain::{
	ControlSource, RunControlAction,
	subnodes::{EndReason, SubnodeId},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::json;
use uuid::Uuid;

/// Fence a Workspace effect against concurrent dissolution in the same
/// transaction as the effect. Non-Subnode Workspaces are unaffected.
pub(crate) async fn require_writable(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	workspace: Uuid,
) -> NativeResult<()> {
	use reinhardt::db::orm::{QueryRow, execution::convert_values};
	let (sql, values) = Query::select()
		.column(Alias::new("state"))
		.from(Alias::new("subnodes"))
		.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
		.lock(LockType::Share)
		.build(PostgresQueryBuilder);
	if let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? {
		let state: String = QueryRow::from_backend_row(row)
			.get("state")
			.ok_or(crate::Error::Forbidden)?;
		if state != "active" {
			return Err(crate::Error::Forbidden);
		}
	}
	Ok(())
}

pub(crate) struct Cascade<'a> {
	pub store: &'a Store,
	pub tx: &'a mut crate::database::native::Transaction,
}
#[async_trait]
impl EndScope for Cascade<'_> {
	async fn active_subnodes(&mut self, creator: Uuid) -> Result<Vec<(SubnodeId, Uuid)>> {
		let result: NativeResult<Vec<(Uuid, Uuid)>> = async {
			crate::database::native::query_as(
				&Query::select()
					.columns([Alias::new("id"), Alias::new("workspace_id")])
					.from(Alias::new("subnodes"))
					.and_where(Expr::col("creator_run_id").eq(Expr::value(creator)))
					.and_where(Expr::col("state").eq("active"))
					.order_by(Alias::new("id"), Order::Asc)
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["id", "workspace_id"])
			.fetch_all(&mut **self.tx)
			.await
		}
		.await;
		Ok(result?
			.into_iter()
			.map(|(id, workspace)| (SubnodeId(id), workspace))
			.collect())
	}
	async fn mark_ended(
		&mut self,
		id: SubnodeId,
		workspace: Uuid,
		reason: EndReason,
	) -> Result<()> {
		let reason = serde_json::to_value(reason)?;
		let reason = reason.as_str().ok_or_else(|| {
			aidash_application::Error::Invalid("invalid Subnode end reason".into())
		})?;
		crate::database::native::query(
			&Query::update()
				.table(Alias::new("subnodes"))
				.value(Alias::new("state"), "dissolved")
				.value_expr(Alias::new("ended_at"), Expr::current_timestamp())
				.value(Alias::new("ended_reason"), reason)
				.and_where(Expr::col("id").eq(Expr::value(id.0)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **self.tx)
		.await?;
		self.store
			.event(
				self.tx,
				Some(workspace),
				"subnode.dissolved",
				json!({"subnode_id":id.0,"reason":reason}),
			)
			.await?;
		Ok(())
	}
	async fn active_runs(&mut self, workspace: Uuid) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("runs"))
					.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
					.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
					.and_where(Expr::col("control").ne("CANCELLED"))
					.order_by(Alias::new("id"), Order::Asc)
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_all(&mut **self.tx)
			.await
		}
		.await;
		Ok(result?)
	}
	async fn control_run(
		&mut self,
		run: Uuid,
		action: RunControlAction,
		source: ControlSource,
	) -> Result<()> {
		self.store
			.control_one_in_as(self.tx, run, action, source)
			.await?;
		Ok(())
	}
}

/// The same cascade when a native compound transaction owns an erased
/// executor rather than Store's concrete transaction wrapper.
pub(crate) struct NativeCascade<'a> {
	pub tx: &'a mut dyn reinhardt::db::backends::TransactionExecutor,
}
#[async_trait]
impl EndScope for NativeCascade<'_> {
	async fn active_subnodes(&mut self, creator: Uuid) -> Result<Vec<(SubnodeId, Uuid)>> {
		use reinhardt::db::orm::{QueryRow, execution::convert_values};
		let (sql, values) = Query::select()
			.columns(["id", "workspace_id"].map(Alias::new))
			.from(Alias::new("subnodes"))
			.and_where(Expr::col("creator_run_id").eq(Expr::value(creator)))
			.and_where(Expr::col("state").eq("active"))
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		let rows = self
			.tx
			.fetch_all(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		rows.into_iter()
			.map(|row| {
				let row = QueryRow::from_backend_row(row);
				let id = row.get("id").ok_or(crate::Error::Forbidden)?;
				let workspace = row.get("workspace_id").ok_or(crate::Error::Forbidden)?;
				Ok((SubnodeId(id), workspace))
			})
			.collect()
	}
	async fn mark_ended(
		&mut self,
		id: SubnodeId,
		workspace: Uuid,
		reason: EndReason,
	) -> Result<()> {
		use reinhardt::db::orm::execution::convert_values;
		let value = serde_json::to_value(reason)?;
		let reason = value.as_str().ok_or_else(|| {
			aidash_application::Error::Invalid("invalid Subnode end reason".into())
		})?;
		let (sql, values) = Query::update()
			.table(Alias::new("subnodes"))
			.value(Alias::new("state"), "dissolved")
			.value_expr(Alias::new("ended_at"), Expr::current_timestamp())
			.value(Alias::new("ended_reason"), reason)
			.and_where(Expr::col("id").eq(Expr::value(id.0)))
			.build(PostgresQueryBuilder);
		self.tx
			.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		crate::apps::execution::models::event_records::append(
			self.tx,
			Some(workspace),
			"subnode.dissolved",
			json!({"subnode_id":id.0,"reason":reason}),
		)
		.await?;
		Ok(())
	}
	async fn active_runs(&mut self, workspace: Uuid) -> Result<Vec<Uuid>> {
		use reinhardt::db::orm::{QueryRow, execution::convert_values};
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.and_where(Expr::col("control").ne("CANCELLED"))
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		let rows = self
			.tx
			.fetch_all(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		rows.into_iter()
			.map(|row| {
				QueryRow::from_backend_row(row)
					.get("id")
					.ok_or(crate::Error::Forbidden)
					.map_err(Into::into)
			})
			.collect()
	}
	async fn control_run(
		&mut self,
		run: Uuid,
		action: RunControlAction,
		source: ControlSource,
	) -> Result<()> {
		use reinhardt::db::orm::{QueryRow, execution::convert_values};
		// This is the cancellation-only branch of Store's control path. It is
		// needed for native multi-party transaction executors, and preserves
		// the same control revision, Safety Stop provenance, preview shutdown,
		// input-ledger fence and run.control event.
		if action != RunControlAction::Cancel {
			return Err(aidash_application::Error::Invalid(
				"native terminal cascade only cancels".into(),
			));
		}
		crate::apps::execution::models::Run::worker_context(self.tx).await?;
		let (sql, values) = Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("control"), "CANCELLED")
			.value(Alias::new("control_source"), source.as_str())
			.value_expr(
				Alias::new("control_revision"),
				Expr::col("control_revision").add(Expr::value(1_i64)),
			)
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.and_where(Expr::col("control").ne("CANCELLED"))
			.returning([Alias::new("workspace_id")])
			.build(PostgresQueryBuilder);
		let row = self
			.tx
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?
			.ok_or_else(|| {
				aidash_application::Error::Conflict("member Run is already terminal".into())
			})?;
		let workspace: Uuid = QueryRow::from_backend_row(row)
			.get("workspace_id")
			.ok_or(crate::Error::Forbidden)?;
		crate::apps::execution::repositories::live_previews::mark_stopped_for_run(
			self.tx,
			run,
			aidash_domain::preview::live::StopReason::RunCancelled,
		)
		.await?;
		crate::apps::execution::models::event_records::append_run(
			self.tx,
			run,
			Some(workspace),
			"run.control",
			json!({"run_id":run,"action":action,"source":source}),
		)
		.await?;
		Ok(())
	}
}
