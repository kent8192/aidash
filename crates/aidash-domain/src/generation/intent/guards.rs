//! Pure binding fences for Home intents and prepared foreign executors.
use super::Intent;
use crate::{Task, federation::execution::Description, generation::requests::Request};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

/// Trusted authority established by an adapter, never deserialized from a request.
#[derive(Clone, Debug)]
pub struct Authority {
	pub tenant: String,
	pub credential_id: Uuid,
	pub subjects: Vec<String>,
}

/// Business projection; the native repository owns its database row type.
#[derive(Clone, Debug)]
pub struct Record {
	pub tenant: String,
	pub credential_id: Uuid,
	pub root_subject: String,
	pub subject_chain: Vec<String>,
	pub binding: Value,
	pub cancelled: bool,
}

pub fn inspection_matches(
	job: &Request,
	authority: &Authority,
	source: &str,
	task: Option<Uuid>,
	now: DateTime<Utc>,
) -> bool {
	job.home_node == source
		&& Some(job.task_id) == task
		&& job.tenant == authority.tenant
		&& job.credential_id == authority.credential_id
		&& job.subject_chain == authority.subjects
		&& job.prepared
		&& matches!(job.status.as_str(), "QUEUED" | "ACTIVE")
		&& job.expires_at > now
}

pub fn binding_matches(
	job: &Request,
	value: &Value,
	intent: &Intent,
	description: &Description,
	admission: Uuid,
	now: DateTime<Utc>,
) -> bool {
	job.foreign_intent.as_ref() == Some(value)
		&& intent.task.id == description.task.id
		&& job.agent_id == description.inspection.agent.id
		&& job.agent_version == description.inspection.agent.version
		&& job.grant_id.is_none_or(|id| id == description.grant_id)
		&& job.admission_id.is_none_or(|id| id == admission)
		&& job.prepared
		&& matches!(job.status.as_str(), "QUEUED" | "ACTIVE")
		&& job.expires_at > now
}

pub fn home_matches(
	record: &Record,
	value: &Value,
	intent: &Intent,
	task: &Task,
	node: &str,
	authority: &Authority,
	now: DateTime<Utc>,
) -> bool {
	let original = &authority.subjects[..authority.subjects.len().saturating_sub(1)];
	!record.cancelled
		&& record.binding == *value
		&& record.credential_id == authority.credential_id
		&& record.tenant == authority.tenant
		&& record.subject_chain == original
		&& intent.target_node == node
		&& intent.task.id == task.id
		&& intent.task.workspace_id == task.workspace_id
		&& intent.expires_at > now
}

impl Intent {
	pub fn preparation_matches(&self, task: &Task) -> bool {
		self.task.id == task.id && self.task.revision == task.revision
	}
}
