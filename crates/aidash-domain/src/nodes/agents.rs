//! Nodes an Agent's chain creates and deletes (#1012, ADR 0143). An Agent has
//! no authority of its own: it creates a Node only when the person at the root
//! of its chain holds `node.create`, and it deletes only a Node its own flow
//! created. Every other Node is deleted by a person, from a browser session.
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
	/// A person, or another flow, created it: even another flow of the same
	/// person.
	NotOwnFlow,
}

/// Whether an Agent may delete a Node that `creator` created: only one that a
/// Run of its own flow created (#1012 decision 20). `creator_flow` is the
/// Root Task of the Task whose Run created the Node, and `own_flow` the Root
/// Task of the deleting Run's Task. A person's Node, or one another flow
/// created, is deleted by that flow's root person or a Tenant Administrator
/// from a browser session, never by an Agent, whatever its root person may
/// do. The default and last live Node rules of
/// [`super::deletion::check_delete`] still apply.
pub fn may_delete(
	creator: &NodeCreator,
	creator_flow: Option<Uuid>,
	own_flow: Uuid,
) -> std::result::Result<(), DeletionRefusal> {
	if matches!(creator, NodeCreator::Agent { .. }) && creator_flow == Some(own_flow) {
		Ok(())
	} else {
		Err(DeletionRefusal::NotOwnFlow)
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

	const FLOW: Uuid = Uuid::from_u128(1);
	const OTHER_FLOW: Uuid = Uuid::from_u128(2);

	fn agent(root: &str) -> NodeCreator {
		NodeCreator::Agent {
			root_subject: root.into(),
			task_id: Uuid::nil(),
			run_id: Uuid::nil(),
		}
	}

	#[rstest]
	#[case::own_flow(agent("alice"), Some(FLOW), true)]
	#[case::another_flow_of_the_same_person(agent("alice"), Some(OTHER_FLOW), false)]
	#[case::unknown_flow(agent("alice"), None, false)]
	#[case::person_created(NodeCreator::Person { identity: None }, Some(FLOW), false)]
	#[case::first_node(NodeCreator::Deployment, Some(FLOW), false)]
	fn an_agent_deletes_only_what_its_own_flow_created(
		#[case] creator: NodeCreator,
		#[case] creator_flow: Option<Uuid>,
		#[case] allowed: bool,
	) {
		// Act
		let decision = may_delete(&creator, creator_flow, FLOW);
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
