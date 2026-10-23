//! Independent Shadow observations retain immutable references and their own leases.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "execution", table_name = "approval_observer_settings")]
#[derive(Serialize, Deserialize)]
pub struct ObserverSetting {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field]
	pub revision: i64,
	#[field]
	pub settings: Json<Value>,
	#[field]
	pub updated_at: DateTime<Utc>,
}

#[model(app_label = "execution", table_name = "approval_observations")]
#[derive(Serialize, Deserialize)]
pub struct ApprovalObservation {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub approval: Uuid,
	#[field(field_type = "text")]
	pub approval_type: String,
	#[field(null = true)]
	pub run_id: Option<Uuid>,
	#[field]
	pub workspace_id: Uuid,
	#[field]
	pub credential_id: Uuid,
	#[field(field_type = "text")]
	pub subject: String,
	#[field]
	pub frame: Json<Value>,
	#[field(null = true)]
	pub evidence: Option<Json<Value>>,
	#[field(field_type = "text")]
	pub settlement: String,
	#[field(null = true)]
	pub worker: Option<Uuid>,
	#[field]
	pub lease_revision: i64,
	#[field(null = true)]
	pub leased_until: Option<DateTime<Utc>>,
	#[field]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "execution", table_name = "approval_observation_attempts")]
#[derive(Serialize, Deserialize)]
pub struct ObservationAttempt {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub observation: Uuid,
	#[field]
	pub decision: Uuid,
	#[field]
	pub record: Json<Value>,
	#[field(field_type = "text")]
	pub status: String,
	#[field(null = true)]
	pub answers: Option<Json<Value>>,
	#[field(null = true)]
	pub provider_report: Option<Json<Value>>,

	#[field]
	pub created_at: DateTime<Utc>,
}

/// Content-free denominator and explicit settlement provenance, including disabled observers.
#[model(
	app_label = "execution",
	table_name = "approval_observation_population"
)]
#[derive(Serialize, Deserialize)]
pub struct ApprovalPopulation {
	#[field(primary_key = true)]
	pub approval: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub approval_type: String,
	#[field(field_type = "text")]
	pub kind: String,
	#[field(null = true)]
	pub observer_unavailable: Option<Json<Value>>,

	#[field(null = true)]
	pub run_id: Option<Uuid>,
	#[field(field_type = "text")]
	pub settlement: String,
	#[field]
	pub created_at: DateTime<Utc>,
}
