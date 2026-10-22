//! Step 3 of a Demo data load: the Registry entries of a Tenant.
use crate::apps::registry::repositories::Registry;
use crate::database::native::Pool;
use crate::registry::Entry;
use crate::{Error, Result};
use aidash_domain::tenancy::TenantName;
use serde_json::Value;

/// The text in an entry's `config` that stands for the Deployment's node id. A
/// Tool descriptor and an Agent's binding targets must name the Node that
/// holds them, which a checked-in structure cannot know.
pub(super) const NODE: &str = "$node";

/// Registers `entries` in order, so an entry's dependencies come first. An
/// identical entry is a no-op; different content under the same id and
/// version fails, and the error names the entry.
pub(super) async fn register(
	pool: &Pool,
	node: &str,
	key: &str,
	tenant: &str,
	entries: &[Entry],
) -> Result<()> {
	let registry = Registry::new(pool.clone(), node)?;
	let tenant = TenantName::new(tenant)?;
	for entry in entries {
		let mut entry = entry.clone();
		bind_node(&mut entry.config, node);
		let (id, version) = (entry.id.clone(), entry.version.clone());
		registry.register(&tenant, entry).await.map_err(|error| {
			Error::Invalid(format!(
				"tenant {key}: registry entry {id}@{version}: {error}"
			))
		})?;
	}
	Ok(())
}

/// Replaces every string equal to [`NODE`] in `value` with `node`.
fn bind_node(value: &mut Value, node: &str) {
	match value {
		Value::String(text) if text == NODE => *text = node.into(),
		Value::Array(items) => items.iter_mut().for_each(|item| bind_node(item, node)),
		Value::Object(members) => members
			.values_mut()
			.for_each(|member| bind_node(member, node)),
		_ => {}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn only_whole_strings_naming_the_node_are_replaced() {
		let mut config = json!({
			"registry_node": "$node",
			"bindings": [{"target": {"registry_node": "$node", "id": "$node-tool"}}],
			"note": "ask $node"
		});
		bind_node(&mut config, "aidash://demo");
		assert_eq!(
			config,
			json!({
				"registry_node": "aidash://demo",
				"bindings": [{"target": {"registry_node": "aidash://demo", "id": "$node-tool"}}],
				"note": "ask $node"
			})
		);
	}
}
