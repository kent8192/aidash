//! Immutable agent revisions preserve attachment identity and catalog approval boundaries.
use crate::registry::bindings::Binding;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configure {
	pub idempotency_key: Uuid,
	pub source_version: String,
	pub new_version: String,
	pub bindings: Vec<Binding>,
	pub remove_default: Vec<String>,
}
