//! Step 3 of a Demo data load: the Registry entries of a Tenant.
use crate::apps::registry::repositories::Registry;
use crate::database::native::Pool;
use crate::registry::Entry;
use crate::{Error, Result};
use aidash_domain::nodes::DeploymentId;
use aidash_domain::tenancy::TenantName;
use serde_json::Value;

/// The text in an entry's `config` that stands for the Deployment id. A
/// Tool descriptor and an Agent's binding targets must name the Deployment that
/// holds them, which a checked-in structure cannot know.
pub(super) const DEPLOYMENT: &str = "$deployment";

/// Registers `entries` in order, so an entry's dependencies come first. An
/// identical entry is a no-op; different content under the same id and
/// version fails, and the error names the entry.
pub(super) async fn register(
	pool: &Pool,
	deployment: &DeploymentId,
	key: &str,
	tenant: &str,
	entries: &[Entry],
) -> Result<()> {
	let registry = Registry::new(pool.clone(), deployment)?;
	let tenant = TenantName::new(tenant)?;
	for entry in entries {
		let mut entry = entry.clone();
		bind_deployment(&mut entry.config, deployment.as_str());
		let (id, version) = (entry.id.clone(), entry.version.clone());
		registry.register(&tenant, entry).await.map_err(|error| {
			Error::Invalid(format!(
				"tenant {key}: registry entry {id}@{version}: {error}"
			))
		})?;
	}
	Ok(())
}

/// Replaces every string equal to [`DEPLOYMENT`] in `value` with `deployment`.
fn bind_deployment(value: &mut Value, deployment: &str) {
	match value {
		Value::String(text) if text == DEPLOYMENT => *text = deployment.into(),
		Value::Array(items) => items
			.iter_mut()
			.for_each(|item| bind_deployment(item, deployment)),
		Value::Object(members) => members
			.values_mut()
			.for_each(|member| bind_deployment(member, deployment)),
		_ => {}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn only_whole_strings_naming_the_deployment_are_replaced() {
		let mut config = json!({
			"registry_deployment": "$deployment",
			"bindings": [{"target": {"registry_deployment": "$deployment", "id": "$deployment-tool"}}],
			"note": "ask $deployment"
		});
		bind_deployment(&mut config, "aidash://demo");
		assert_eq!(
			config,
			json!({
				"registry_deployment": "aidash://demo",
				"bindings": [{"target": {"registry_deployment": "aidash://demo", "id": "$deployment-tool"}}],
				"note": "ask $deployment"
			})
		);
	}
}
