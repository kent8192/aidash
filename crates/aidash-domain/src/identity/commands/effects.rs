//! Remote audit events carry admission-bound identifiers rather than protected content.
use crate::{Error, Result, Task, TaskStatus};
use serde_json::{Value, json};
use uuid::Uuid;
pub fn field<'a>(data: &'a Value, name: &str) -> Result<&'a str> {
	data[name]
		.as_str()
		.ok_or_else(|| Error::Invalid(format!("missing {name}")))
}
pub fn child_eligible(parent: &Task, child: &Task, owner: &str) -> bool {
	child.workspace_id == parent.workspace_id
		&& child.parent_id == Some(parent.id)
		&& child.created_by == owner
		&& child.owner.is_none()
		&& child.status == TaskStatus::Open
}
#[derive(Debug)]
pub enum AuditError {
	Unbound,
	Invalid(Error),
}
impl From<Error> for AuditError {
	fn from(error: Error) -> Self {
		Self::Invalid(error)
	}
}
pub fn audit_event(
	data: &Value,
	admission: Uuid,
) -> std::result::Result<(&'static str, Value), AuditError> {
	let payload = &data["data"];
	if payload["run_id"] != json!(admission) {
		return Err(AuditError::Unbound);
	}
	match field(data, "kind")? {
		"remote.run.recovered" => {
			let phase = field(payload, "phase")?;
			let cause = field(payload, "cause")?;
			if phase.len() > 32 || cause != "expired worker lease" {
				return Err(Error::Invalid("invalid recovery event".into()).into());
			}
			Ok((
				"task.remote_run_recovered",
				json!({"phase":phase,"cause":cause}),
			))
		}
		"remote.tool.completed" => {
			let call = &payload["call"];
			let name = field(call, "name")?;
			let id = field(call, "id")?;
			if name.len() > 256 || id.len() > 256 {
				return Err(Error::Invalid("invalid tool event".into()).into());
			}
			Ok((
				"task.remote_tool_completed",
				json!({"call":{"id":id,"name":name}}),
			))
		}
		_ => Err(AuditError::Unbound),
	}
}
#[cfg(test)]
mod tests;
