//! Reporter-supplied incidents and immutable copied evidence, distinct from a Trust assessment.
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Serialize, JsonSchema)]
pub struct IncidentEvent {
	pub id: i64,
	pub incident_id: Uuid,
	pub actor: String,
	pub change: Value,
	pub created_at: DateTime<Utc>,
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

pub fn validate(
	severity: &str,
	status: &str,
	notes: &str,
	evidence: &[EvidenceInput],
) -> crate::Result<()> {
	if !matches!(severity, "low" | "medium" | "high" | "critical")
		|| !matches!(status, "open" | "resolved")
		|| notes.trim().is_empty()
		|| notes.len() > 16_384
		|| evidence.len() > 8
		|| evidence
			.iter()
			.map(|item| item.content.len())
			.sum::<usize>()
			> 65_536
		|| evidence.iter().any(|item| {
			item.title.trim().is_empty() || item.title.len() > 255 || item.content.trim().is_empty()
		}) {
		return Err(crate::Error::Invalid(
			"invalid incident severity, status, notes or evidence".into(),
		));
	}
	Ok(())
}
pub fn fixed_copy(input: EvidenceInput, recorded_at: DateTime<Utc>) -> EvidenceCopy {
	use sha2::{Digest, Sha256};
	EvidenceCopy {
		sha256: format!("{:x}", Sha256::digest(input.content.as_bytes())),
		title: input.title,
		content: Some(input.content),
		recorded_at,
	}
}
#[cfg(test)]
mod tests;
