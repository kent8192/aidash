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
	if matches!(
		operation,
		"delegate"
			| "task_assign"
			| "connection_request"
			| "outbound_request"
			| "memory_mutate"
			| "goal_verdict"
	) && data["key"].as_str().is_none()
	{
		return Err(Error::Invalid("missing command key".into()));
	}
	let mutation = matches!(
		operation,
		"claim"
			| "transition"
			| "run_message_terminal_transition"
			| "run_message_complete"
			| "goal_verdict"
			| "child_goal"
			| "run_abandon_unassigned"
			| "create_task"
			| "delegate"
			| "message"
			| "run_message_output"
			| "event" | "run_queued"
			| "human_request"
			| "mcp.connection_call"
			| "task_assign"
			| "connection_request"
			| "outbound_request"
			| "memory_mutate"
	);
	let digest = if operation == "run_message_complete" {
		registry::rules::digest(
			&json!({"operation":operation,"run_id":data["run_id"],"settlement_version":data["settlement_version"]}),
		)
	} else if operation == "goal_verdict" {
		registry::rules::digest(&json!({
		 "operation": operation,
		 "run_id": data["run_id"],
		 "response_epoch": data["verdict"]["response_epoch"],
		 "goal": data["verdict"]["goal"],
		}))
	} else {
		registry::rules::digest(&json!({"operation":operation,"data":data}))
	};
	// Personal calls use their logical invocation key. `not_personal` is an internal
	// fallback outcome, and pre-execution connection causes must never enter replay.
	let key = if operation == "mcp.connection_call" {
		Some(
			data["idempotency_key"]
				.as_str()
				.ok_or_else(|| Error::Invalid("missing idempotency_key".into()))?,
		)
	} else {
		data["key"].as_str()
	};
	let request_key = if operation == "run_queued" {
		// A single notice belongs to this grant, even if a different command
		// key is supplied by a peer. The typed payload accepts no such key.
		operation.to_owned()
	} else if let Some(key) = key {
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
		"human_request" | "human_read" => Some("human_request"),
		"thread_record" | "thread_record_chunk" => Some("thread_read"),
		"message" => Some("thread_message"),
		"create_task" => Some("task_create"),
		"delegate" | "child_goal" => Some("task_delegate"),
		"discover" => Some("agent_discover"),
		"created_task" => Some("task_delegate"),
		// Home-forwarded tools (ADR 0139): each op is named as its tool so the
		// Binding and the Home's own Request ([`requested`]) are checked for the
		// tool the Run called.
		"task_assign" => Some("task_assign"),
		"thread_search" => Some("thread_search"),
		"connection_request" => Some("connection_request"),
		// A remote Run's outbound GET is approved by the Home's root person first (ADR 0139,
		// decision 22): the Home asks and answers under the tool the Run called.
		"outbound_request" | "outbound_read" => Some("outbound_get"),
		"memory_recall" => Some("memory_recall"),
		"memory_reflect" => Some("memory_reflect"),
		"memory_mutate" => Some("memory_mutate"),
		_ => None,
	}
}
/// Whether the Home checks its own Federation Tool Request for `operation` before it serves it
/// (ADR 0139, decision 1): the Home-forwarded tools, whose ops a receiving Node sends only when
/// the Run called that tool. The older ops also carry traffic of the Run itself that no list
/// names (a person asked about a tool approval or a Completion Review, records read for the
/// Run's context, tasks an Agent tool creates), so only the Binding is checked for them.
/// `delegate` and `discover` come only from a remote Run's `task_delegate` and `agent_discover`,
/// which hand work on through the Home (#654, #1013), so they are checked too. `created_task`
/// only tells the receiver whether its grant made a Task, before the lists decide the call.
pub fn requested(operation: &str) -> bool {
	matches!(
		operation,
		"delegate"
			| "discover"
			| "task_assign"
			| "thread_search"
			| "connection_request"
			| "outbound_request"
			| "outbound_read"
			| "memory_recall"
			| "memory_reflect"
			| "memory_mutate"
	)
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

pub mod connections;
pub mod effects;
