//! Persistence inventory for native memory provenance, receipts and bounded maintenance.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "knowledge", table_name = "memory_receiver_caches")]
#[derive(Serialize, Deserialize)]
pub struct MemoryReceiverCache {
	#[field(primary_key = true)]
	pub run_id: Uuid,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field]
	pub invalidated: bool,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub attempts: i32,
	#[field]
	pub max_attempts: i32,
	#[field]
	pub next_attempt: DateTime<Utc>,
	#[field(null = true, field_type = "text")]
	pub last_error: Option<String>,
}

#[model(app_label = "knowledge", table_name = "memory_remote_reads")]
#[derive(Serialize, Deserialize)]
pub struct MemoryRemoteRead {
	#[field(primary_key = true)]
	pub grant_id: Uuid,
	#[field(primary_key = true)]
	pub unit_id: Uuid,
	#[field(primary_key = true)]
	pub revision: i64,
}
#[model(app_label = "knowledge", table_name = "memory_remote_read_gates")]
#[derive(Serialize, Deserialize)]
pub struct MemoryRemoteReadGate {
	#[field(primary_key = true)]
	pub grant_id: Uuid,
}

#[model(app_label = "knowledge", table_name = "memory_history")]
#[derive(Serialize, Deserialize)]
pub struct MemoryHistory {
	#[field(primary_key = true)]
	pub unit_id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field(primary_key = true)]
	pub revision: i64,
	#[field]
	pub operation_id: Uuid,
	#[field(field_type = "text")]
	pub actor: String,
	#[field(field_type = "text")]
	pub text: String,
	#[field(field_type = "text")]
	pub kind: String,
	#[field(field_type = "text")]
	pub learning: String,
	#[field(field_type = "text")]
	pub verification: String,
	#[field(null = true)]
	pub occurred_start: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub occurred_end: Option<DateTime<Utc>>,
	#[field]
	pub entities: Json<Value>,
	#[field]
	pub evidence: Json<Value>,
	#[field]
	pub links: Json<Value>,
	#[field]
	pub learned_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field]
	pub deleted: bool,
	#[field]
	pub stale: bool,
	#[field(null = true)]
	pub mental_model: Option<Json<Value>>,
}

#[model(app_label = "knowledge", table_name = "memory_receipts")]
#[derive(Serialize, Deserialize)]
pub struct MemoryReceipt {
	#[field(primary_key = true)]
	pub operation_id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub outcome: Json<Value>,
	#[field]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "knowledge", table_name = "memory_dependencies")]
#[derive(Serialize, Deserialize)]
pub struct MemoryDependency {
	#[field(primary_key = true)]
	pub unit_id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub source_kind: String,
	#[field(primary_key = true)]
	pub source_id: Uuid,
	#[field(primary_key = true)]
	pub source_revision: i64,
}

#[model(app_label = "knowledge", table_name = "memory_candidates")]
#[derive(Serialize, Deserialize)]
pub struct MemoryCandidate {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field]
	pub revision: i64,
	#[field]
	pub run: Json<Value>,
	#[field(field_type = "text")]
	pub text: String,
	#[field(field_type = "text")]
	pub kind: String,
	#[field(field_type = "text")]
	pub learning: String,
	#[field(field_type = "text")]
	pub verification: String,
	#[field(null = true)]
	pub occurred_start: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub occurred_end: Option<DateTime<Utc>>,
	#[field]
	pub entities: Json<Value>,
	#[field]
	pub evidence: Json<Value>,
	#[field]
	pub links: Json<Value>,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field(null = true)]
	pub mental_model: Option<Json<Value>>,
}

#[model(app_label = "knowledge", table_name = "memory_model_operations")]
#[derive(Serialize, Deserialize)]
pub struct MemoryModelOperation {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field(field_type = "text")]
	pub provider_id: String,
	#[field(field_type = "text")]
	pub provider_version: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub calls: i64,
	#[field]
	pub tokens: i64,
	#[field]
	pub cost_micros: i64,
	#[field]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "knowledge", table_name = "memory_model_attempts")]
#[derive(Serialize, Deserialize)]
pub struct MemoryModelAttempt {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub operation_id: Uuid,
	#[field]
	pub ordinal: i32,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub tokens: i64,
	#[field]
	pub cost_micros: i64,
	#[field(null = true)]
	pub output: Option<Json<Value>>,
	#[field]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "knowledge", table_name = "memory_publications")]
#[derive(Serialize, Deserialize)]
pub struct MemoryPublication {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field]
	pub source_id: Uuid,
	#[field]
	pub source_revision: i64,
	#[field]
	pub revision: i64,
	#[field]
	pub authority: Json<Value>,
	#[field]
	pub deleted: bool,
	#[field]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "knowledge", table_name = "memory_task_participants")]
#[derive(Serialize, Deserialize)]
pub struct MemoryTaskParticipant {
	#[field(primary_key = true)]
	pub task_id: Uuid,
	#[field]
	pub participant_id: Uuid,
	#[field]
	pub participant_revision: i64,
}

#[model(app_label = "knowledge", table_name = "memory_run_reads")]
#[derive(Serialize, Deserialize)]
pub struct MemoryRunRead {
	#[field(primary_key = true)]
	pub run_id: Uuid,
	#[field(primary_key = true)]
	pub unit_id: Uuid,
	#[field(primary_key = true)]
	pub revision: i64,
}

#[model(app_label = "knowledge", table_name = "memory_run_read_gates")]
#[derive(Serialize, Deserialize)]
pub struct MemoryRunReadGate {
	#[field(primary_key = true)]
	pub run_id: Uuid,
}

#[model(app_label = "knowledge", table_name = "memory_candidate_reviews")]
#[derive(Serialize, Deserialize)]
pub struct MemoryCandidateReview {
	#[field(primary_key = true)]
	pub operation_id: Uuid,
	#[field]
	pub candidate_id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field]
	pub observed_revision: i64,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(field_type = "text")]
	pub actor: String,
	#[field]
	pub outcome: Json<Value>,
	#[field]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "knowledge", table_name = "memory_purge_jobs")]
#[derive(Serialize, Deserialize)]
pub struct MemoryPurgeJob {
	#[field(primary_key = true)]
	pub unit_id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field]
	pub revision: i64,
	#[field]
	pub purge_after: DateTime<Utc>,
	#[field]
	pub backup_until: DateTime<Utc>,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field(default = 0)]
	pub attempts: i32,
	#[field]
	pub next_attempt: DateTime<Utc>,
	#[field(field_type = "text", null = true)]
	pub last_error: Option<String>,
}

#[model(app_label = "knowledge", table_name = "memory_bank_settings")]
#[derive(Serialize, Deserialize)]
pub struct MemoryBankSettings {
	#[field(primary_key = true)]
	pub bank_id: Uuid,
	#[field(field_type = "text")]
	pub provider_id: String,
	#[field(field_type = "text")]
	pub provider_version: String,
	#[field]
	pub revision: i64,
	#[field]
	pub next_maintenance: DateTime<Utc>,
}

#[model(app_label = "knowledge", table_name = "memory_engine_jobs")]
#[derive(Serialize, Deserialize)]
pub struct MemoryEngineJob {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field(field_type = "text")]
	pub provider_id: String,
	#[field(field_type = "text")]
	pub provider_version: String,
	#[field(field_type = "text")]
	pub kind: String,
	#[field]
	pub input: Json<Value>,
	#[field]
	pub authority: Json<Value>,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub attempts: i32,
	#[field(null = true)]
	pub claim: Option<Uuid>,
	#[field]
	pub next_attempt: DateTime<Utc>,
	#[field(field_type = "text", null = true)]
	pub last_error: Option<String>,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
}

#[model(app_label = "knowledge", table_name = "memory_learning_discovery")]
#[derive(Serialize, Deserialize)]
pub struct MemoryLearningDiscovery {
	#[field(primary_key = true, field_type = "text")]
	pub home: String,
	#[field]
	pub cursor_updated: DateTime<Utc>,
	#[field]
	pub cursor_id: Uuid,
	#[field]
	pub cycle_before: DateTime<Utc>,
}

#[model(app_label = "knowledge", table_name = "memory_bank_policy_changes")]
#[derive(Serialize, Deserialize)]
pub struct MemoryBankPolicyChange {
	#[field(primary_key = true)]
	pub operation_id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub applied_revision: i64,
	#[field]
	pub actor: Json<Value>,
	#[field]
	pub created_at: DateTime<Utc>,
}
