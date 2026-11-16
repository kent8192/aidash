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
	/// Allowlisted actual provider facts; NULL means unknown, never zero billing.
	#[field(null = true)]
	pub provider_report: Option<Json<Value>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub finished_at: Option<DateTime<Utc>>,
}

/// One immutable Decision Evidence snapshot.
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
