use serde::{Deserialize, Serialize};
// Serializable authority contracts.

#[derive(
	Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct Origin {
	pub(crate) credential_id: Uuid,
	pub(crate) tenant: String,
	pub(crate) subject: String,
}

/// No state, artifact content, registry document or provider argument is sent
/// during preflight. Resource identifiers and the recipient set are explicit.
#[derive(
	Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
	pub(crate) kind: String,
	pub(crate) id: Uuid,
	pub(crate) task_id: Option<Uuid>,
}

#[derive(
	Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preflight {
	pub(crate) id: Uuid,
	pub(crate) coordinator: String,
	pub(crate) digest: String,
	pub(crate) origin: Origin,
	pub(crate) recipients: Vec<String>,
	pub(crate) targets: Vec<Target>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub(crate) struct Binding {
	pub(crate) request: Preflight,
	pub(crate) local: Origin,
	pub(crate) subjects: Vec<String>,
}

use uuid::Uuid;
