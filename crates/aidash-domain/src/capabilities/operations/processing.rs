//! Committed operation receipts and storage retry disclosure are housekeeping rules.
use serde_json::{Value, json};
use uuid::Uuid;
pub struct Receipt {
	pub id: Uuid,
	pub digest: String,
	pub runner_instance: Option<String>,
	pub uncertain: bool,
}
/// Canonical JSON contains only the output retained by this exact runner
/// operation. An ACK proves adoption of this snapshot, never mere termination.
pub fn output_digest(instance: &str, observed: &Value) -> crate::Result<String> {
	use sha2::{Digest, Sha256};
	let snapshot = json!({"runner_instance":instance,"operation_id":observed["operation_id"],
		"digest":observed["digest"],"stdout":observed["stdout"],"files":observed["files"],
		"displays":observed["displays"],"truncated":observed["truncated"]});
	Ok(format!(
		"{:x}",
		Sha256::digest(serde_json::to_vec(&snapshot)?)
	))
}
pub fn journal_lost(previous: Option<&str>, health: &Value) -> bool {
	previous.is_some_and(|old| {
		health["instance"]
			.as_str()
			.is_some_and(|current| current != old)
	})
}
pub fn storage_blockage(message: &str) -> Option<Value> {
	(message.starts_with("STORAGE_QUOTA")||message=="runner file quota"||message=="runner output quota").then(||json!({"code":"STORAGE_QUOTA","message":"Storage quota prevents saving this result. Restore capacity to reconcile the same operation; prior files remain intact.","retryable":true}))
}
#[cfg(test)]
mod tests;
