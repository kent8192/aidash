//! Persistent memory records.

use crate::Result;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Json, Model};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[model(app_label = "execution", table_name = "memory")]
#[derive(Serialize, Deserialize)]
pub struct Memory {
	#[field(primary_key = true, field_type = "text")]
	pub agent_id: String,
	#[field(primary_key = true, field_type = "text")]
	pub agent_version: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub workspace_id: uuid::Uuid,
	#[field]
	pub data: Json<Value>,
	#[field(primary_key = true, field_type = "text")]
	pub home_node: String,
}

impl Memory {
	pub(crate) async fn for_run(
		tx: &mut dyn TransactionExecutor,
		agent: &str,
		version: &str,
		workspace: Uuid,
		home: &str,
	) -> Result<Value> {
		Ok(Self::objects()
			.filter(Self::field_agent_id().eq(agent))
			.filter(Self::field_agent_version().eq(version))
			.filter(Self::field_workspace_id().eq(workspace))
			.filter(Self::field_home_node().eq(home))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map_or_else(|| json!({}), |record| record.data.into_inner()))
	}
}
