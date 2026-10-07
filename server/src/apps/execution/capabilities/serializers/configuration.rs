use serde::{Deserialize, Serialize};
// Serializable configuration contracts.
use aidash_domain::registry::bindings::Binding;

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Configure {
	pub idempotency_key: Uuid,
	#[validate(length(min = 1, max = 128))]
	pub source_version: String,
	#[validate(length(min = 1, max = 128))]
	pub new_version: String,
	pub bindings: Vec<Binding>,
	pub remove_default: Vec<String>,
}

use uuid::Uuid;
