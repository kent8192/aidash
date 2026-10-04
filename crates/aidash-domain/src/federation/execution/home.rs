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
	pub grant: Prepared,
	pub execution: Option<Activation>,
	pub unavailable: bool,
	pub semantic: crate::semantic::remote::status::Status,
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
