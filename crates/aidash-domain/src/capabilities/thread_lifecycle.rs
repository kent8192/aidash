use serde::{Deserialize, Serialize};
// Serializable thread lifecycle contracts.
use super::{cleanup::Choice, sessions::Area};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileChoice {
	pub area_id: Uuid,
	pub expected_revision: i64,
	pub choice: Choice,
	pub confirmation_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteThread {
	pub idempotency_key: Uuid,
	pub files: Vec<FileChoice>,
}

use uuid::Uuid;

/// Deletion choices cover every owned Area exactly once.
pub fn choices(
	files: Vec<FileChoice>,
) -> crate::Result<std::collections::BTreeMap<Uuid, FileChoice>> {
	let mut choices = std::collections::BTreeMap::new();
	for choice in files {
		if choices.insert(choice.area_id, choice).is_some() {
			return Err(crate::Error::Invalid("DUPLICATE_FILE_CHOICE".into()));
		}
	}
	Ok(choices)
}
pub fn require_idle(area: &Area, revision: i64) -> crate::Result<()> {
	if area.revision != revision {
		return Err(crate::Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	if !matches!(
		area.state.as_str(),
		"active" | "retained" | "recoverable" | "deleted"
	) {
		return Err(crate::Error::Conflict(
			"AREA_BUSY: stop and reconcile execution before deleting the thread".into(),
		));
	};
	Ok(())
}
#[cfg(test)]
mod tests;
