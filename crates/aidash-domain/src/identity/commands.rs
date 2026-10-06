//! Scoped command identity and durable replay metadata retain their original namespaces.
use crate::{Error, Result, registry};
use serde_json::{Value, json};
use uuid::Uuid;
pub struct Binding {
	pub admission_id: Uuid,
	pub task_id: Uuid,
}
pub struct Metadata {
	pub mutation: bool,
	pub digest: String,
	pub request_key: String,
}
pub fn prepare(operation: &str, data: &Value) -> Result<Metadata> {
	if operation == "delegate" && data["key"].as_str().is_none() {
		return Err(Error::Invalid("missing command key".into()));
	}
	let mutation =
		matches!(
			operation,
			"claim"
				| "transition"
				| "run_message_terminal_transition"
				| "run_message_complete"
				| "artifact" | "create_task"
				| "delegate" | "message"
				| "run_message_output"
				| "event"
		);
	let digest = registry::rules::digest(&json!({"operation":operation,"data":data}));
	let request_key = if let Some(key) = data["key"].as_str() {
		if key.is_empty() || key.len() > 512 {
			return Err(Error::Invalid("invalid command key".into()));
		}
		format!("{operation}:{key}")
	} else {
		format!("{operation}:{}", data["revision"])
	};
	Ok(Metadata {
		mutation,
		digest,
		request_key,
	})
}
pub fn builtin(operation: &str) -> Option<&'static str> {
	match operation {
		"artifact" => Some("artifact_publish"),
		"message" => Some("workspace_message"),
		"create_task" => Some("task_create"),
		"delegate" => Some("task_delegate"),
		_ => None,
	}
}
pub fn require_input_key(key: &str, run: Uuid) -> Result<()> {
	if key.len() > 512
		|| !(key.starts_with(&format!("human:{run}:"))
			|| (key.starts_with("subject-human:")
				&& key.rsplit(':').nth(1) == Some(run.to_string().as_str())))
	{
		return Err(Error::Invalid("invalid scoped run message key".into()));
	}
	Ok(())
}
#[cfg(test)]
mod tests;

pub mod effects;
