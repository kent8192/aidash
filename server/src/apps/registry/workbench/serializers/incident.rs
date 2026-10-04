use serde::{Deserialize, Serialize};
// Serializable contracts for workbench.

use crate::apps::registry::workbench::models::{AgentIncident, AgentIncidentEvent};
use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceInput {
	pub title: String,
	pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceCopy {
	pub title: String,
	pub content: Option<String>,
	pub sha256: String,
	pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, JsonSchema)]
pub struct Incident {
	pub id: Uuid,
	pub tenant: String,
	pub agent_id: String,
	pub version: String,
	pub revision: i64,
	pub severity: String,
	pub status: String,
	pub archived: bool,
	pub owner: String,
	pub notes: String,
	pub evidence: Value,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub resolved_at: Option<DateTime<Utc>>,
	pub evidence_expires_at: Option<DateTime<Utc>>,
	pub evidence_expired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, sqlx::FromRow, JsonSchema)]
pub struct IncidentEvent {
	pub id: i64,
	pub incident_id: Uuid,
	pub actor: String,
	pub change: Value,
	pub created_at: DateTime<Utc>,
}

impl From<AgentIncident> for Incident {
	fn from(row: AgentIncident) -> Self {
		Self {
			id: row.id,
			tenant: row.tenant,
			agent_id: row.agent_id,
			version: row.version,
			revision: row.revision,
			severity: row.severity,
			status: row.status,
			archived: row.archived,
			owner: row.owner,
			notes: row.notes,
			evidence: row.evidence.into_inner(),
			created_at: row.created_at,
			updated_at: row.updated_at,
			resolved_at: row.resolved_at,
			evidence_expires_at: row.evidence_expires_at,
			evidence_expired_at: row.evidence_expired_at,
		}
	}
}

impl From<AgentIncidentEvent> for IncidentEvent {
	fn from(row: AgentIncidentEvent) -> Self {
		Self {
			id: row.id,
			incident_id: row.incident_id(),
			actor: row.actor,
			change: row.change.into_inner(),
			created_at: row.created_at,
		}
	}
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateIncident {
	pub tenant: Option<String>,
	pub severity: String,
	pub owner: String,
	pub notes: String,
	#[serde(default)]
	pub evidence: Vec<EvidenceInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateIncident {
	pub expected_revision: i64,
	pub severity: String,
	pub status: String,
	pub archived: Option<bool>,
	pub owner: String,
	pub notes: String,
	#[serde(default)]
	pub add_evidence: Vec<EvidenceInput>,
}

use uuid::Uuid;
