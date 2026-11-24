//! Immutable transaction manifests and monotonic coordinator decisions.
use crate::{ArtifactInput, Error, Result, nodes::NodeAddress};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionIsolation")]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
	Serializable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionMutation")]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mutation {
	/// Rename a Thread (its only mutable attribute).
	ThreadTitle {
		thread_id: Uuid,
		expected_revision: i64,
		title: String,
	},
	CompleteTask {
		task_id: Uuid,
		expected_revision: i64,
		artifact: ArtifactInput,
	},
	FinishRun {
		run_id: Uuid,
		task_id: Uuid,
		expected_revision: i64,
	},
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionParticipant")]
#[serde(deny_unknown_fields)]
pub struct Participant {
	pub node_id: String,
	pub mutations: Vec<Mutation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionManifest")]
#[serde(deny_unknown_fields)]
pub struct Manifest {
	pub id: Uuid,
	pub coordinator: String,
	pub isolation: Isolation,
	pub deadline: DateTime<Utc>,
	pub participants: Vec<Participant>,
}
#[derive(Debug, thiserror::Error)]
#[error("forbidden")]
pub struct MissingParticipant;
impl Manifest {
	pub fn validate<E>(&self) -> std::result::Result<(), E>
	where
		E: From<Error> + From<serde_json::Error>,
	{
		if self.id.is_nil() {
			return Err(
				Error::Invalid("transaction requires an ID and 1..16 participants".into()).into(),
			);
		}
		validate_nodes(
			&self.coordinator,
			&self
				.participants
				.iter()
				.map(|participant| participant.node_id.as_str())
				.collect::<Vec<_>>(),
		)?;
		let mut count = 0;
		for participant in &self.participants {
			count += validate_mutations(&participant.mutations)?;
		}
		if count == 0 || serde_json::to_vec(self)?.len() > 524_288 {
			return Err(Error::Invalid("transaction needs mutations, its coordinator as a participant, and a manifest within 512 KiB".into()).into());
		}
		Ok(())
	}
	/// The participant that is exactly the Node at `address`. Another Node of
	/// the same Deployment, such as another Tenant's, is never the same
	/// participant.
	pub fn local(&self, address: &str) -> std::result::Result<&Participant, MissingParticipant> {
		self.participants
			.iter()
			.find(|p| p.node_id == address)
			.ok_or(MissingParticipant)
	}
}
/// The coordinator and the ordered participant Nodes of one transaction.
/// A Deployment takes part with one Node, so a Deployment-level answer to
/// "does this transaction run here" names exactly one Node.
pub(crate) fn validate_nodes(coordinator: &str, nodes: &[&str]) -> Result<()> {
	NodeAddress::parse(coordinator)?;
	if nodes.is_empty() || nodes.len() > 16 {
		return Err(Error::Invalid(
			"transaction requires an ID and 1..16 participants".into(),
		));
	}
	let mut hosts = std::collections::BTreeMap::new();
	for node in std::iter::once(&coordinator).chain(nodes) {
		if hosts
			.insert(crate::nodes::deployment_of(node), *node)
			.is_some_and(|other| other != *node)
		{
			return Err(Error::Invalid(
				"a transaction names at most one Node of each Deployment".into(),
			));
		}
	}
	let mut previous: Option<&str> = None;
	for node in nodes {
		NodeAddress::parse(node)?;
		if previous.is_some_and(|p| p >= *node) {
			return Err(Error::Invalid(
				"participants must be unique, sorted by node ID, with at most 64 mutations each"
					.into(),
			));
		}
		previous = Some(node);
	}
	if !nodes.contains(&coordinator) {
		return Err(Error::Invalid("transaction needs mutations, its coordinator as a participant, and a manifest within 512 KiB".into()));
	}
	Ok(())
}

/// One participant's mutations; returns how many there are.
pub(crate) fn validate_mutations(mutations: &[Mutation]) -> Result<usize> {
	if mutations.len() > 64 {
		return Err(Error::Invalid(
			"participants must be unique, sorted by node ID, with at most 64 mutations each".into(),
		));
	}
	let mut keys = std::collections::BTreeSet::new();
	for mutation in mutations {
		let key = match mutation {
			Mutation::ThreadTitle {
				thread_id,
				expected_revision,
				title,
			} => {
				if *expected_revision < 0 || title.trim().is_empty() {
					return Err(Error::Invalid(
						"thread mutation requires a revision and a title".into(),
					));
				}
				format!("thread:{thread_id}")
			}
			Mutation::CompleteTask {
				task_id,
				expected_revision,
				artifact,
			} => {
				if *expected_revision < 0 {
					return Err(Error::Invalid("task revision must be nonnegative".into()));
				}
				artifact.validate()?;
				format!("task:{task_id}")
			}
			Mutation::FinishRun {
				run_id,
				expected_revision,
				..
			} => {
				if *expected_revision < 0 {
					return Err(Error::Invalid("run revision must be nonnegative".into()));
				}
				format!("run:{run_id}")
			}
		};
		if !keys.insert(key) {
			return Err(Error::Invalid(
				"a resource can be mutated only once per manifest".into(),
			));
		}
	}
	Ok(mutations.len())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinatorDecision {
	Commit,
	Abort,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoordinatorState {
	pub decision: Option<CoordinatorDecision>,
	pub visible: bool,
	pub complete: bool,
}
#[derive(Clone, Copy)]
pub enum CoordinatorTransition {
	Decide(CoordinatorDecision),
	Publish,
	Complete,
}
impl CoordinatorTransition {
	pub fn phase(&self) -> &'static str {
		match self {
			Self::Decide(CoordinatorDecision::Commit) => "COMMIT",
			Self::Decide(CoordinatorDecision::Abort) => "ABORT",
			Self::Publish => "VISIBLE",
			Self::Complete => "COMPLETE",
		}
	}
	pub fn apply(&self, state: &mut CoordinatorState) -> crate::Result<bool> {
		match self {
			Self::Decide(decision) => {
				if state.decision.is_some() {
					return Ok(false);
				}
				state.decision = Some(*decision);
			}
			Self::Publish => {
				if state.decision != Some(CoordinatorDecision::Commit) {
					return Err(Error::Conflict(
						"only a committed transaction can become visible".into(),
					));
				}
				if state.visible {
					return Ok(false);
				}
				state.visible = true;
			}
			Self::Complete => {
				match state.decision {
					Some(CoordinatorDecision::Abort) => {}
					Some(CoordinatorDecision::Commit) if state.visible => {}
					_ => {
						return Err(Error::Conflict(
							"transaction must have a finalized decision".into(),
						));
					}
				}
				if state.complete {
					return Ok(false);
				}
				state.complete = true;
			}
		}
		Ok(true)
	}
}
#[cfg(test)]
mod tests;

pub mod authority;
pub mod coordination;
pub mod management;
pub mod mutation;
pub mod share;
