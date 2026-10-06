//! Native ORM records are converted into portable incident values here.
use crate::apps::registry::workbench::models::{AgentIncident, AgentIncidentEvent};
pub use aidash_domain::registry::workbench::incident::{
	CreateIncident, EvidenceCopy, EvidenceInput, Incident, IncidentEvent, UpdateIncident,
};
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
