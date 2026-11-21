//! Nodes an Agent's chain creates and deletes with the `core.nodes@1` tools
//! (#1012, ADR 0143). The Home acts as the person at the root of the Run's
//! chain: creating needs that person's `node.create`, and deleting needs a
//! Node a chain of the same person created, or that person's `node.delete`.

/// The arguments of the Node tools, as the model sees their schemas.
pub mod tool {
	use schemars::JsonSchema;
	use serde::Deserialize;

	#[derive(Debug, Clone, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct Create {
		/// What people call the new Node, 1 to 64 characters, such as "Research".
		pub label: String,
	}

	#[derive(Debug, Clone, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct Delete {
		/// The name of the Node to delete, as `node_create` returned it.
		pub node: String,
	}
}
