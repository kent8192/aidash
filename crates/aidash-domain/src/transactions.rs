//! Immutable transaction manifests and monotonic coordinator decisions.
use crate::{ArtifactInput, Error, configuration::validate_node_id, registry::Entry};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionIsolation")]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
	Serializable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionMutation")]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mutation {
	RegistryRegister {
		entry: Box<Entry>,
	},
	WorkspaceState {
		workspace_id: Uuid,
		expected_revision: i64,
		state: Value,
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
	pub fn validate_with<E>(
		&self,
		mut validate_registry: impl FnMut(&Entry) -> std::result::Result<(), E>,
	) -> std::result::Result<(), E>
	where
		E: From<Error> + From<serde_json::Error>,
	{
		validate_node_id(&self.coordinator)?;
		if self.id.is_nil() || self.participants.is_empty() || self.participants.len() > 16 {
			return Err(
				Error::Invalid("transaction requires an ID and 1..16 participants".into()).into(),
			);
		}
		let mut previous: Option<&str> = None;
		let mut count = 0;
		for participant in &self.participants {
			validate_node_id(&participant.node_id)?;
			if previous.is_some_and(|p| p >= participant.node_id.as_str())
				|| participant.mutations.len() > 64
			{
				return Err(Error::Invalid("participants must be unique, sorted by node ID, with at most 64 mutations each".into()).into());
			}
			previous = Some(&participant.node_id);
			let mut keys = std::collections::BTreeSet::new();
			for mutation in &participant.mutations {
				let key = match mutation {
					Mutation::RegistryRegister { entry } => {
						validate_registry(entry)?;
						format!("registry:{}@{}", entry.id, entry.version)
					}
					Mutation::WorkspaceState {
						workspace_id,
						expected_revision,
						state,
					} => {
						if *expected_revision < 0 || !state.is_object() {
							return Err(Error::Invalid(
								"workspace mutation requires a revision and object state".into(),
							)
							.into());
						}
						format!("workspace:{workspace_id}")
					}
					Mutation::CompleteTask {
						task_id,
						expected_revision,
						artifact,
					} => {
						if *expected_revision < 0 {
							return Err(
								Error::Invalid("task revision must be nonnegative".into()).into()
							);
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
							return Err(
								Error::Invalid("run revision must be nonnegative".into()).into()
							);
						}
						format!("run:{run_id}")
					}
				};
				if !keys.insert(key) {
					return Err(Error::Invalid(
						"a resource can be mutated only once per manifest".into(),
					)
					.into());
				}
			}
			count += participant.mutations.len();
		}
		if count == 0
			|| !self
				.participants
				.iter()
				.any(|p| p.node_id == self.coordinator)
			|| serde_json::to_vec(self)?.len() > 524_288
		{
			return Err(Error::Invalid("transaction needs mutations, its coordinator as a participant, and a manifest within 512 KiB".into()).into());
		}
		Ok(())
	}
	pub fn digest(&self) -> std::result::Result<String, serde_json::Error> {
		Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
	}
	pub fn local(&self, node: &str) -> std::result::Result<&Participant, MissingParticipant> {
		self.participants
			.iter()
			.find(|p| p.node_id == node)
			.ok_or(MissingParticipant)
	}
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
pub mod mutation;
