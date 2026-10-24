//! Committed operation receipts and storage retry disclosure are housekeeping rules.
use serde_json::{Value, json};
use uuid::Uuid;
pub struct Receipt {
	pub id: Uuid,
	pub digest: String,
	pub runner_instance: Option<String>,
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
