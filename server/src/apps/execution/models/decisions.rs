//! Decision Evidence and the charged attempts behind it. Every row belongs to one
//! Run and never references a Registry row: the Decider is recorded by its exact pin.
use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// One charged request to a Decider's provider, keyed by its stable attempt
/// identity. `pending` means reserved and possibly dispatched; recovery reports
/// it as uncertain and never dispatches it again.
#[model(app_label = "execution", table_name = "decision_attempts")]
#[derive(Serialize, Deserialize)]
pub struct DecisionAttempt {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub decision_id: Uuid,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "decision_attempt_run",
		db_index = false,
		on_delete = Cascade,
		on_update = Restrict
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text")]
	pub node: String,
	#[field]
	pub step: i32,
	/// The Run's resume cycle the decision identity commits to.
	#[field]
	pub cycle: i32,
	/// The step's outage retry the decision identity commits to (ADR 0066).
	#[field]
	pub retry: i32,
	#[field]
	pub input_revision: i64,
	#[field(field_type = "text")]
	pub input_digest: String,
	/// The lease that reserved the attempt; a later worker only recovers it.
	#[field]
	pub worker: Uuid,
	#[field]
	pub run_revision: i64,
	#[field]
	pub decider: Json<Value>,
	#[field]
	pub binding_restrictions: Json<Value>,
	/// The Decider Route the attempt asked (ADR 0066).
	#[field]
	pub route: i32,
	#[field(field_type = "text")]
	pub request_digest: String,
	#[field(field_type = "text")]
	pub state_digest: String,
	#[field]
	pub questions: Json<Value>,
	/// Commits to the definitions of `questions`, which the request carried.
	#[field(field_type = "text")]
	pub question_digest: String,
	#[field(field_type = "text")]
	pub mode: String,
	#[field]
	pub owner_receipts: Json<Value>,
	#[field(field_type = "text")]
	pub status: String,
	#[field(null = true)]
	pub answers: Option<Json<Value>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub finished_at: Option<DateTime<Utc>>,
}

/// One immutable Decision Evidence snapshot. A decision has at most one applied,
/// staged or shadow snapshot; each retry of a rejected decision appends another.
#[model(app_label = "execution", table_name = "decision_evidence")]
#[derive(Serialize, Deserialize)]
pub struct DecisionEvidence {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub decision_id: Uuid,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "decision_evidence_run",
		db_index = false,
		on_delete = Cascade,
		on_update = Restrict
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text")]
	pub outcome: String,
	#[field(field_type = "text")]
	pub reason: String,
	#[field]
	pub evidence: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

/// The classification state a Decider version asked to keep. Retention cleanup
/// deletes the row and leaves an expired marker in its evidence.
#[model(app_label = "execution", table_name = "decision_states")]
#[derive(Serialize, Deserialize)]
pub struct DecisionState {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub evidence_id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field]
	pub state: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

/// An appended observation after a decision, never a causal claim about it.
#[model(app_label = "execution", table_name = "decision_outcomes")]
#[derive(Serialize, Deserialize)]
pub struct DecisionOutcome {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub decision_id: Uuid,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "decision_outcome_run",
		db_index = false,
		on_delete = Cascade,
		on_update = Restrict
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text", null = true)]
	pub next_inference: Option<String>,
	#[field(field_type = "text")]
	pub observed_status: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
