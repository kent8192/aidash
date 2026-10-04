//! Durable execution bindings retain the admitted subject chain and credential fence.
use crate::{RunMetadata, qualified_agent};
use uuid::Uuid;

/// Authenticated authority metadata, never constructed by request deserialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPrincipal {
	pub tenant: String,
	pub subject: String,
	pub credential_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionGrant {
	pub run_id: Uuid,
	pub task_id: Uuid,
	pub workspace_id: Uuid,
	pub tenant: String,
	pub credential_id: Uuid,
	pub root_subject: String,
	pub subject_chain: Vec<String>,
}
impl ExecutionGrant {
	fn agent_matches(&self, node: &str, run: &RunMetadata) -> bool {
		self.subject_chain.first() == Some(&self.root_subject)
			&& self.subject_chain.last()
				== Some(&qualified_agent(node, &run.agent_id, &run.agent_version))
	}
	pub fn matches_worker(&self, node: &str, run: &RunMetadata) -> bool {
		run.home_node == node
			&& self.run_id == run.id
			&& self.task_id == run.task_id
			&& self.workspace_id == run.workspace_id
			&& self.agent_matches(node, run)
	}
	pub fn matches_inherited(&self, identity: &ExecutionPrincipal, run: &RunMetadata) -> bool {
		self.tenant == identity.tenant
			&& self.root_subject == identity.subject
			&& self.task_id == run.task_id
			&& self.workspace_id == run.workspace_id
			&& self.agent_matches(&run.home_node, run)
	}
	pub fn matches_refreshed(
		&self,
		node: &str,
		identity: &ExecutionPrincipal,
		run: &RunMetadata,
	) -> bool {
		self.run_id == run.id
			&& self.task_id == run.task_id
			&& self.workspace_id == run.workspace_id
			&& self.tenant == identity.tenant
			&& self.root_subject == identity.subject
			&& self.credential_id == identity.credential_id
			&& self.agent_matches(node, run)
	}
	/// Credential rotation and chain changes force a retry at the authority boundary.
	pub fn same_source(&self, initial: &Self) -> bool {
		self.credential_id == initial.credential_id && self.subject_chain == initial.subject_chain
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOrigin {
	pub tenant: String,
	pub root_subject: String,
	pub subject_chain: Vec<String>,
}
impl TaskOrigin {
	pub fn compatible(&self, identity: &ExecutionPrincipal, current: &[String]) -> bool {
		self.tenant == identity.tenant
			&& self.root_subject == identity.subject
			&& (current.len() <= 1 || current == self.subject_chain)
	}
}
impl From<(String, String, Vec<String>)> for TaskOrigin {
	fn from((tenant, root_subject, subject_chain): (String, String, Vec<String>)) -> Self {
		Self {
			tenant,
			root_subject,
			subject_chain,
		}
	}
}

/// A task's original producer and authority remain immutable across retries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedTaskOrigin {
	pub source_run_id: Uuid,
	pub authority: TaskOrigin,
}

#[cfg(test)]
mod tests;
