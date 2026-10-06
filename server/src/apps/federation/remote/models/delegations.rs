//! Persistent delegations records.

use crate::apps::workspaces::models::Task;
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "federation", table_name = "delegations")]
#[derive(Serialize, Deserialize)]
pub struct Delegation {
	#[field(primary_key = true)]
	pub task_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub node_id: String,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub agent_version: String,
	#[field(default = false)]
	pub delivered: bool,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub next_attempt_at: DateTime<Utc>,
}

use crate::apps::execution::models::event_records;
use crate::apps::federation::remote::serializers::runtime::Delegation as DelegationContract;
use crate::apps::workspaces::models::states::TaskStatus;
use crate::apps::workspaces::serializers::entities::Task as TaskContract;
use crate::domain::qualified_agent;
use crate::registry::EntityRef;
use crate::{Error, Result};
use chrono::Duration;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{DatabaseConnection, Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::json;

impl Delegation {
	pub(crate) async fn node_for_task(
		tx: &mut dyn TransactionExecutor,
		task: Uuid,
	) -> Result<Option<String>> {
		Ok(Self::objects()
			.filter(Self::field_task_id().eq(task))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|row| row.node_id))
	}

	pub(crate) async fn reserve(
		tx: &mut dyn TransactionExecutor,
		local_node: &str,
		expected: &TaskContract,
		node: &str,
		agent: &EntityRef,
	) -> Result<(TaskContract, DelegationContract)> {
		let mut task = Task::objects()
			.filter(Task::field_id().eq(expected.id))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or_else(|| Error::NotFound("task".into()))?;
		let owner = qualified_agent(node, &agent.id, &agent.version);
		if task.revision != expected.revision
			|| (task.owner.is_some() && task.owner.as_deref() != Some(&owner))
			|| (task.status != TaskStatus::Open && task.owner.as_deref() != Some(&owner))
		{
			return Err(Error::Conflict(
				"task changed or is already assigned".into(),
			));
		}
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(["task_id", "node_id", "agent_id", "agent_version"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(task.id),
				IntoValue::into_value(node),
				IntoValue::into_value(&agent.id),
				IntoValue::into_value(&agent.version),
			])
			.on_conflict(OnConflict::columns(["task_id"]).do_nothing().to_owned())
			.build(PostgresQueryBuilder);
		let inserted = tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			> 0;
		if inserted && task.status == TaskStatus::Open {
			// Reserve the claimant and invalidate pre-delegation revisions while
			// dependency waiting still leaves the task OPEN.
			task.revision += 1;
			let (sql, values) = Query::update()
				.table(Alias::new(Task::table_name()))
				.value_expr(Alias::new("revision"), Expr::value(task.revision))
				.and_where(Expr::col("id").eq(Expr::value(task.id)))
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		let delegation = Self::objects()
			.filter(Self::field_task_id().eq(task.id))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or_else(|| Error::NotFound("delegation".into()))?;
		if delegation.node_id != node
			|| delegation.agent_id != agent.id
			|| delegation.agent_version != agent.version
		{
			return Err(Error::Conflict(
				"task already delegated to a different agent".into(),
			));
		}
		let delegation: DelegationContract = delegation.into();
		if inserted {
			event_records::append(
				tx,
				local_node,
				Some(task.workspace_id),
				"task.delegated",
				json!(delegation),
			)
			.await?;
		}
		Ok((task.into(), delegation))
	}

	pub(crate) async fn mark_delivered<E: OrmExecutor>(db: &mut E, task: Uuid) -> Result<()> {
		Self::objects()
			.filter(Self::field_task_id().eq(task))
			.update_fields_with_conn(db, [(Self::field_delivered(), true)])
			.await?;
		Ok(())
	}

	pub(crate) async fn authorized<E: OrmExecutor>(
		db: &mut E,
		task: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<bool> {
		Ok(Self::objects()
			.filter(Self::field_task_id().eq(task))
			.filter(Self::field_node_id().eq(node))
			.filter(Self::field_agent_id().eq(&agent.id))
			.filter(Self::field_agent_version().eq(&agent.version))
			.exists_with_db(db)
			.await?)
	}

	pub(crate) async fn claim_retries(db: DatabaseConnection) -> Result<Vec<DelegationContract>> {
		db.atomic(async |tx| {
			let (sql, values) = Query::select()
				.expr_as(Expr::current_timestamp(), Alias::new("now"))
				.build(PostgresQueryBuilder);
			let now: DateTime<Utc> =
				TransactionExecutor::fetch_one(tx, &sql, convert_values(values))
					.await?
					.get("now")
					.map_err(FrameworkError::from)?;
			let rows = Self::objects()
				.filter(Self::field_delivered().eq(false))
				.filter(Self::field_next_attempt_at().lte(now))
				.order_by(&["next_attempt_at", "created_at", "task_id"])
				.limit(100)
				.select_for_update()
				.skip_locked()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?;
			if !rows.is_empty() {
				Self::objects()
					.filter(Self::field_task_id().is_in(rows.iter().map(|row| row.task_id)))
					.update_fields_with_conn(
						tx,
						[(Self::field_next_attempt_at(), now + Duration::seconds(5))],
					)
					.await?;
			}
			Ok(rows.into_iter().map(Into::into).collect())
		})
		.await
	}
}

use reinhardt::query::IntoValue;

impl Delegation {}
