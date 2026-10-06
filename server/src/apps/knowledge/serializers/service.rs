use serde::{Deserialize, Serialize};
// Serializable contracts for semantic.

use schemars::JsonSchema;

/// Only constructed from authenticated in-process identities; never accepted
/// from API input. Persisted jobs retain the initiator and delegation chain.
#[derive(Serialize, Deserialize, JsonSchema)]
pub(crate) struct SavedAuthority {
	pub(crate) credential: Option<Uuid>,
	pub(crate) tenant: String,
	pub(crate) subject: String,
	pub(crate) subjects: Vec<String>,
}

use uuid::Uuid;
