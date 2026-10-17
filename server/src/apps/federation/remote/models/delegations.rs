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
	#[field]
	pub human_requests: reinhardt::db::orm::Json<serde_json::Value>,
	#[field]
	pub binding_snapshot:
		Option<reinhardt::db::orm::Json<aidash_domain::registry::bindings::ForeignAgentSnapshot>>,
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
use reinhardt::db::orm::{Model, OrmExecutor};
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
}

use reinhardt::query::IntoValue;

impl Delegation {}
