//! Nodes an Agent's chain creates and deletes (#1012, ADR 0143). An Agent has
//! no authority of its own: it creates a Node only when the person at the root
//! of its chain holds `node.create`, and it deletes a Node that a chain of the
//! same person created, or any Node when that person holds `node.delete`.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The longest key an Agent's tool call names a Node creation with.
pub const MAX_CREATION_KEY_BYTES: usize = 512;

/// Who created a Node, as the org chart shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeCreator {
	/// A person, through a browser session (or the first Node of their Tenant).
	Person {
		/// The External Identity, when one is recorded.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		identity: Option<Uuid>,
	},
	/// A Deployment Administrator or Host Access, for a Tenant's first Node.
	Deployment,
	/// An Agent's chain, acting as its root person, from a Run of a Task.
	Agent {
		root_subject: String,
		task_id: Uuid,
		run_id: Uuid,
	},
}

impl NodeCreator {
	/// Reads the stored creator columns of a Node.
	pub fn from_columns(
		kind: &str,
		identity: Option<Uuid>,
		root_subject: Option<String>,
		task: Option<Uuid>,
		run: Option<Uuid>,
	) -> Result<Self> {
		Ok(match kind {
			"identity" => Self::Person { identity },
			"deployment_administrator" | "host_access" => Self::Deployment,
			"agent" => match (root_subject, task, run) {
				(Some(root_subject), Some(task_id), Some(run_id)) => Self::Agent {
					root_subject,
					task_id,
					run_id,
				},
				_ => {
					return Err(Error::Invalid(
						"an Agent-created Node names its root person, Task and Run".into(),
					));
				}
			},
			other => return Err(Error::Invalid(format!("unknown Node creator: {other}"))),
		})
	}

	/// The root person whose chain created the Node, if an Agent's did.
	pub fn root_subject(&self) -> Option<&str> {
		match self {
			Self::Agent { root_subject, .. } => Some(root_subject),
			_ => None,
		}
	}
}

/// Why an Agent's chain may not delete a Node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionRefusal {
	/// Another person's chain, or a person, created it, and the root person
	/// holds no `node.delete`.
	NotOwnChain,
}

/// Whether an Agent's chain acting as `root_person` may delete a Node that
/// `creator` created: one a chain of the same person created, or any Node
/// when that person holds `node.delete` (#1012). The default and last live
/// Node rules of [`super::deletion::check_delete`] still apply.
pub fn may_delete(
	creator: &NodeCreator,
	root_person: &str,
	holds_node_delete: bool,
) -> std::result::Result<(), DeletionRefusal> {
	if holds_node_delete || creator.root_subject() == Some(root_person) {
		Ok(())
	} else {
		Err(DeletionRefusal::NotOwnChain)
	}
}

/// Validates the key a retried tool call finds its Node by.
pub fn creation_key(key: &str) -> Result<&str> {
	if key.is_empty() || key.len() > MAX_CREATION_KEY_BYTES || key.contains('\0') {
		return Err(Error::Invalid(
			"a Node creation key is 1 to 512 bytes without NUL".into(),
		));
	}
	Ok(key)
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	fn agent(root: &str) -> NodeCreator {
		NodeCreator::Agent {
			root_subject: root.into(),
			task_id: Uuid::nil(),
			run_id: Uuid::nil(),
		}
	}

	#[rstest]
	#[case::own_chain(agent("alice"), "alice", false, true)]
	#[case::other_person_chain(agent("bob"), "alice", false, false)]
	#[case::other_person_chain_with_node_delete(agent("bob"), "alice", true, true)]
	#[case::person_created(NodeCreator::Person { identity: None }, "alice", false, false)]
	#[case::person_created_with_node_delete(NodeCreator::Person { identity: None }, "alice", true, true)]
	#[case::first_node(NodeCreator::Deployment, "alice", false, false)]
	fn an_agent_deletes_only_what_its_root_person_chains_created(
		#[case] creator: NodeCreator,
		#[case] root: &str,
		#[case] holds_node_delete: bool,
		#[case] allowed: bool,
	) {
		// Act
		let decision = may_delete(&creator, root, holds_node_delete);
		// Assert
		assert_eq!(decision.is_ok(), allowed);
	}

	#[rstest]
	#[case("identity", true)]
	#[case("deployment_administrator", true)]
	#[case("host_access", true)]
	#[case("agent", true)]
	#[case("robot", false)]
	fn stored_creator_kinds_are_read(#[case] kind: &str, #[case] known: bool) {
		// Arrange
		let task = Uuid::new_v4();
		let run = Uuid::new_v4();
		// Act
		let creator =
			NodeCreator::from_columns(kind, None, Some("alice".into()), Some(task), Some(run));
		// Assert
		assert_eq!(creator.is_ok(), known);
		if kind == "agent" {
			assert_eq!(
				creator.unwrap(),
				NodeCreator::Agent {
					root_subject: "alice".into(),
					task_id: task,
					run_id: run
				}
			);
		}
	}

	#[test]
	fn an_agent_creator_without_its_task_is_refused() {
		// Act
		let creator = NodeCreator::from_columns("agent", None, Some("alice".into()), None, None);
		// Assert
		assert!(creator.is_err());
	}

	#[rstest]
	#[case("", false)]
	#[case("call-1", true)]
	#[case("a\0b", false)]
	fn creation_keys_are_bounded(#[case] key: &str, #[case] valid: bool) {
		assert_eq!(creation_key(key).is_ok(), valid);
	}

	#[test]
	fn creation_keys_longer_than_the_bound_are_refused() {
		let key = "k".repeat(MAX_CREATION_KEY_BYTES + 1);
		assert!(creation_key(&key).is_err());
		assert!(creation_key(&key[..MAX_CREATION_KEY_BYTES]).is_ok());
	}
}
