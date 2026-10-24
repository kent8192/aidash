use serde::Deserialize;
// Serializable broker contracts.

#[derive(Deserialize)]
pub(crate) struct Hint {
	pub(crate) specversion: String,
	pub(crate) source: String,
	pub(crate) id: Uuid,
	pub(crate) subject: Option<Uuid>,
}

use uuid::Uuid;
