//! Metadata retained by transaction preflight and admitted subject chains.
use crate::{RunMetadata, identity::execution::ExecutionPrincipal};
use uuid::Uuid;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
	pub credential_id: Uuid,
	pub tenant: String,
	pub subject: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
	pub kind: String,
	pub id: Uuid,
	pub task_id: Option<Uuid>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preflight {
	pub id: Uuid,
	pub coordinator: String,
	pub digest: String,
	pub origin: Origin,
	pub recipients: Vec<String>,
	pub targets: Vec<Target>,
}

/// An immutable reservation records both source and local subject authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
	pub request: Preflight,
	pub local: Origin,
	pub subjects: Vec<String>,
}

/// Coordinator facts used by admission and control, independent of an ORM row.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
	pub id: Uuid,
	pub digest: String,
	pub manifest: serde_json::Value,
	pub decision: Option<String>,
	pub visible: bool,
	pub complete: bool,
	pub last_error: Option<String>,
	pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<&ExecutionPrincipal> for Origin {
	fn from(identity: &ExecutionPrincipal) -> Self {
		Self {
			credential_id: identity.credential_id,
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		}
	}
}
impl Preflight {
	pub fn permits(&self, caller: &str, node: &str) -> bool {
		caller == self.coordinator
			&& self.targets.len() <= 64
			&& !self.recipients.is_empty()
			&& self.recipients.len() <= 16
			&& self.recipients.iter().any(|recipient| recipient == node)
			&& self.digest.len() == 64
	}
}
pub struct SourceAdmission {
	pub tenant: String,
	pub credential_id: Uuid,
	pub subject_chain: Vec<String>,
	pub source_tenant: String,
	pub source_subject: String,
	pub workspace_id: Uuid,
	pub agent_id: String,
	pub agent_version: String,
}
impl SourceAdmission {
	pub fn matches(
		&self,
		identity: &ExecutionPrincipal,
		origin: &Origin,
		run: &RunMetadata,
	) -> bool {
		self.tenant == identity.tenant
			&& self.credential_id == identity.credential_id
			&& self.source_tenant == origin.tenant
			&& self.source_subject == origin.subject
			&& self.workspace_id == run.workspace_id
			&& self.agent_id == run.agent_id
			&& self.agent_version == run.agent_version
			&& self.subject_chain.first() == Some(&identity.subject)
	}
}
pub fn chain_compatible(previous: &[String], current: &[String]) -> bool {
	previous.len() <= 1 || previous == current
}
#[cfg(test)]
mod tests;
