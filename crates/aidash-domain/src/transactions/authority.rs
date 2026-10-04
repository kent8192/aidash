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
