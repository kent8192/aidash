//! Durable replay precedes mutation ownership and terminal-state checks.
use crate::{Error, Result, ports::authorization::commands::RemoteCommandScope};
use aidash_domain::{
	Task, TaskStatus,
	identity::commands::{self, Metadata},
	policy::Resource,
	qualified_agent,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub struct Prepared {
	pub task: Task,
	pub owner: String,
	pub metadata: Metadata,
	pub key: String,
	pub task_resource: Resource,
}
pub enum Admission {
	Replay(Value),
	Ready(Box<Prepared>),
}
pub struct Command<'a> {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub operation: &'a str,
	pub data: &'a Value,
	pub node: &'a str,
	pub agent: &'a str,
	pub version: &'a str,
}
pub async fn prepare(scope: &mut dyn RemoteCommandScope, input: Command<'_>) -> Result<Admission> {
	let bound = scope
		.binding(input.grant_id)
		.await?
		.ok_or(Error::Forbidden)?;
	if bound.admission_id != input.admission_id {
		return Err(Error::Forbidden);
	}
	if let Some(run) = input.data.get("run_id")
		&& *run != json!(input.admission_id)
	{
		return Err(Error::Forbidden);
	}
	let task = scope.task(bound.task_id).await?;
	let owner = qualified_agent(input.node, input.agent, input.version);
	let metadata = commands::prepare(input.operation, input.data)?;
	if metadata.mutation {
		if let Some((old, result)) = scope
			.previous(input.grant_id, &metadata.request_key)
			.await?
		{
			if old != metadata.digest {
				return Err(Error::Conflict(
					"remote command key binds different input".into(),
				));
			}
			return Ok(Admission::Replay(result));
		}
		if task.owner.as_deref().is_some_and(|saved| saved != owner) {
			return Err(Error::Forbidden);
		}
		if matches!(
			task.status,
			TaskStatus::Completed
				| TaskStatus::Failed
				| TaskStatus::Cancelled
				| TaskStatus::Abandoned
		) {
			return Err(Error::Conflict("remote task is terminal".into()));
		}
	}
	let key = format!("scoped:{}:{}", input.grant_id, metadata.request_key);
	let task_resource = scope.task_resource(&task).await?;
	if let Some(tool) = commands::builtin(input.operation) {
		scope.require_builtin(tool).await?;
	}
	Ok(Admission::Ready(Box::new(Prepared {
		task,
		owner,
		metadata,
		key,
		task_resource,
	})))
}
#[cfg(test)]
mod tests;

pub mod effects;
