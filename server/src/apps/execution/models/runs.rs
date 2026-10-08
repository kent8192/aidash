//! Persistent runs records.

use crate::Result;
use crate::apps::execution::services::states::RunControl;
use crate::apps::execution::services::states::RunPhase;
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::DatabaseConnection;
use reinhardt::db::orm::{Json, Model as ModelTrait, OrmExecutor};
use reinhardt::macros::Model;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Model, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[model_config(app_label = "execution", table_name = "runs")]
pub struct Run {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub task_id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub home_node: String,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub agent_version: String,
	#[field(field_type = "text", max_length = 64)]
	pub phase: RunPhase,
	#[field(field_type = "text", max_length = 64)]
	pub control: RunControl,
	#[field]
	pub context: Json<Value>,
	#[field]
	pub pending: Json<Value>,
	#[field(default = 0)]
	pub step: i32,
	#[field(default = 0)]
	pub revision: i64,
	#[field(field_type = "text", null = true)]
	pub error: Option<String>,
	#[field(null = true)]
	pub lease_owner: Option<uuid::Uuid>,
	#[field(null = true)]
	pub lease_until: Option<DateTime<Utc>>,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
	#[field(default = 0)]
	pub observed_input_seq: i64,
	#[field(default = false)]
	pub ledger_worker_ready: bool,

	/// Local continuations retain their owning-Run FK; remote continuations live at Home.
	/// DDL exception: SchemaExpr cannot express JSON extraction and CASE yet.
	#[field(
		null = true,
		generated_sql = "
CASE
    WHEN ((context -> 'binding_snapshot'::text) -> 'remote'::text) = 'true'::jsonb THEN NULL::uuid
    WHEN ((((pending -> 'data'::text) ->> 'reason'::text) = ANY (ARRAY['human'::text, 'external_approval'::text, 'reconciliation'::text])) AND (jsonb_typeof(((pending -> 'data'::text) -> 'request_id'::text)) = 'string'::text) AND (((pending -> 'data'::text) ->> 'request_id'::text) ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) THEN (((pending -> 'data'::text) ->> 'request_id'::text))::uuid
    ELSE NULL::uuid
END",
		generated_stored = true
	)]
	pub pending_human_request_id: Option<uuid::Uuid>,
}

impl Run {
	pub(crate) async fn mark_identity_disabled<E: OrmExecutor>(
		db: &mut E,
		ids: Vec<Uuid>,
	) -> Result<()> {
		if !ids.is_empty() {
			Self::objects()
				.filter(Self::field_id().is_in(ids))
				.filter(Self::field_control().eq(RunControl::Paused))
				.filter(Self::field_error().eq(Some("identity status unavailable".to_owned())))
				.update_fields_with_conn(
					db,
					[(
						Self::field_error(),
						Some("external identity disabled".to_owned()),
					)],
				)
				.await?;
		}
		Ok(())
	}

	/// The caller retains its credential and policy lease through this commit.
	pub(crate) async fn resume_identity_pause(db: DatabaseConnection, id: Uuid) -> Result<bool> {
		db.atomic(async |tx| {
			let row = Self::objects()
				.filter(Self::field_id().eq(id))
				.filter(Self::field_control().eq(RunControl::Paused))
				.filter(Self::field_error().eq(Some("identity status unavailable".to_owned())))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop();
			let Some(mut row) = row else {
				return Ok(false);
			};
			row.control = RunControl::Active;
			row.error = None;
			row.revision += 1;
			row.updated_at = Utc::now();
			Self::objects().update_with_conn(tx, &row).await?;
			Ok(true)
		})
		.await
	}
}
