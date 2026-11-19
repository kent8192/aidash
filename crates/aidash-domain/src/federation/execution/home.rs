//! Source grants and Home bindings retain authored task identity independently of database rows.
use super::{Inspection, Prepared, admission::Activation};
use crate::registry::EntityRef;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone, Debug)]
pub struct Grant {
	pub id: Uuid,
	pub task_id: Uuid,
	pub task_revision: i64,
	pub workspace_id: Uuid,
	pub node_id: String,
	pub tenant: String,
	pub credential_id: Uuid,
	pub root_subject: String,
	pub subject_chain: Vec<String>,
	pub inspection: Value,
	pub expires_at: DateTime<Utc>,
	pub revoked: bool,
	pub semantic: Value,
}
impl Grant {
	pub fn prepared(&self) -> serde_json::Result<Prepared> {
		let inspection: Inspection = serde_json::from_value(self.inspection.clone())?;
		Ok(Prepared {
			id: self.id,
			task_id: self.task_id,
			node_id: self.node_id.clone(),
			agent: EntityRef {
				id: inspection.agent.id,
				version: inspection.agent.version,
			},
			expires_at: self.expires_at,
			revoked: self.revoked,
			semantic: serde_json::from_value(self.semantic.clone())?,
		})
	}
}
#[derive(Clone, Debug)]
pub struct HomeBinding {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub task_id: Uuid,
	pub task_revision: i64,
	pub initial_task: Value,
	/// The Home recorded the receiver's activation of the Run (#772, #1013).
	pub activated: bool,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowUpInput {
	pub id: Uuid,
	pub title: String,
	pub description: String,
	pub requirements: crate::registry::Search,
}
pub struct Status {
	pub human_requests: Vec<crate::HumanRequest>,
	pub grant: Prepared,
	pub execution: Option<Activation>,
	pub unavailable: bool,
	pub semantic: crate::semantic::remote::status::Status,
	pub requester: bool,
}
impl HomeBinding {
	pub fn matches(&self, admission: Uuid, task: Uuid, initial_task: &Value) -> bool {
		self.admission_id == admission && self.task_id == task && &self.initial_task == initial_task
	}
}
#[cfg(test)]
mod tests;

impl Grant {
	/// Reusing a preparation key requires every pinned authority and authored definition to match.
	pub fn matches_authority(&self, expected: &PreparationAuthority<'_>) -> bool {
		self.task_id == expected.task_id
			&& self.task_revision == expected.task.revision
			&& self.workspace_id == expected.task.workspace_id
			&& self.node_id == expected.input.node_id
			&& self.credential_id == expected.identity.credential_id
			&& self.tenant == expected.identity.tenant
			&& self.root_subject == expected.identity.subject
			&& self.subject_chain == expected.subjects
			&& &self.inspection == expected.inspection
			&& &self.semantic == expected.semantic
	}
	/// Current commands may advance the revision journal while receiver input retains its original image.
	pub fn admitted_task(
		&self,
		task: &crate::Task,
		binding: Option<&HomeBinding>,
	) -> serde_json::Result<Option<crate::Task>> {
		let expected = binding.map_or(self.task_revision, |bound| bound.task_revision);
		if task.revision != expected
			|| task.workspace_id != self.workspace_id
			|| (binding.is_none() && task.status != crate::TaskStatus::Open)
		{
			return Ok(None);
		}
		if let Some(bound) = binding {
			let original: crate::Task = serde_json::from_value(bound.initial_task.clone())?;
			if bound.grant_id != self.id
				|| bound.task_id != task.id
				|| original.id != task.id
				|| original.workspace_id != task.workspace_id
				|| original.revision != self.task_revision
			{
				return Ok(None);
			}
			Ok(Some(original))
		} else {
			Ok(Some(task.clone()))
		}
	}
}

/// What decides whether a Remote Grant still lets its Task be worked
/// (#1013, Round 3 Q5 of the agent activation grilling).
///
/// A grant's lifetime (at most an hour) is its admission deadline: it bounds
/// the grant only until the receiver has started the Task. From then on the
/// grant lives while its Task has not ended, whether the Run's Stint is awake
/// or asleep waiting for an approval, CI or a child Task, so the Home keeps it
/// renewed for as long as the Task is open. Revocation ends it at once. A broken
/// chain is not part of this rule: the Home re-evaluates the whole chain at
/// every use, so every use of such a grant is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liveness {
	pub revoked: bool,
	pub expires_at: DateTime<Utc>,
	/// This grant's own Home binding recorded the receiver's activation. The
	/// Task's status never stands in for it.
	pub activated: bool,
	/// The receiver's executor holds the Task: the Task's owner is the last
	/// subject of the grant's chain, which only that receiver's claim makes it.
	/// A Task someone else claimed, or one a retry reopened, is not held.
	pub held: bool,
	/// The Home Task's status, if the Task still exists.
	pub task: Option<crate::TaskStatus>,
}

impl Liveness {
	/// The facts of `grant`, its Home binding and its Home Task.
	pub fn of(grant: &Grant, binding: Option<&HomeBinding>, task: Option<&crate::Task>) -> Self {
		Self {
			revoked: grant.revoked,
			expires_at: grant.expires_at,
			activated: binding.is_some_and(|binding| {
				binding.grant_id == grant.id
					&& binding.task_id == grant.task_id
					&& binding.activated
			}),
			held: task.is_some_and(|task| {
				task.id == grant.task_id
					&& task.owner.is_some()
					&& task.owner.as_ref() == grant.subject_chain.last()
			}),
			task: task.map(|task| task.status),
		}
	}

	/// Whether the grant is live at `now`: inside its lifetime, or activated,
	/// held by its receiver and its Task not ended.
	pub fn live(&self, now: DateTime<Utc>) -> bool {
		!self.revoked
			&& (self.expires_at > now
				|| (self.activated
					&& self.held && self.task.is_some_and(|status| !status.is_terminal())))
	}
}

/// The Task statuses that end a Remote Grant its receiver holds.
pub fn ending_statuses() -> impl Iterator<Item = crate::TaskStatus> {
	[
		crate::TaskStatus::Open,
		crate::TaskStatus::Claimed,
		crate::TaskStatus::Running,
		crate::TaskStatus::Completed,
		crate::TaskStatus::Failed,
		crate::TaskStatus::Blocked,
		crate::TaskStatus::Cancelled,
		crate::TaskStatus::Abandoned,
	]
	.into_iter()
	.filter(|status| status.is_terminal())
}

/// Complete current authority required to reuse an immutable preparation key.
pub struct PreparationAuthority<'a> {
	pub task_id: Uuid,
	pub task: &'a crate::Task,
	pub input: &'a super::PrepareInput,
	pub identity: &'a crate::identity::execution::ExecutionPrincipal,
	pub subjects: &'a [String],
	pub inspection: &'a Value,
	pub semantic: &'a Value,
}
