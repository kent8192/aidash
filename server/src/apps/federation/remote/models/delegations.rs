//! Persistent delegations records.

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
	pub tenant: String,
	#[field(field_type = "text")]
	pub node_id: String,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub agent_version: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

use crate::Result;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;

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

impl Delegation {}
