//! Canonical content identities and immutable package invariants.
use super::{DependencyBinding, Installation, Version};
use crate::{
	Error, Result,
	registry::{ClusterConfig, EntityRef, Entry, Package},
	tool::ToolConfig,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
pub fn key(value: &impl Serialize) -> String {
	crate::registry::rules::digest(&serde_json::to_value(value).expect("serializable identity"))
		.trim_start_matches("sha256:")
		.to_string()
}
pub fn reference(entry: &Entry) -> EntityRef {
	EntityRef {
		id: entry.id.clone(),
		version: entry.version.clone(),
	}
}
pub fn content(entry: &Entry) -> String {
	let mut entry = entry.clone();
	entry.id.clear();
	entry.version.clear();
	entry.installation = None;
	entry.binding_normalization = None;
	key(&entry)
}
pub fn rewrite(entry: &mut Entry, bindings: &[DependencyBinding], node: &str) -> Result<()> {
	fn bind(reference: &mut EntityRef, bindings: &[DependencyBinding]) -> bool {
		if let Some(binding) = bindings.iter().find(|b| b.source == *reference) {
			*reference = binding.target.clone();
			return true;
		}
		false
	}
	match entry.kind.as_str() {
		"agent" => {
			let mut c: crate::registry::bindings::AgentBindings =
				serde_json::from_value(entry.config.clone())?;
			bind(&mut c.model, bindings);
			for reference in c.cluster.iter_mut() {
				bind(reference, bindings);
			}
			for bound in &mut c.bindings {
				let mut local = bound.target.local();
				if bind(&mut local, bindings) {
					bound.target.registry_node = node.into();
					bound.target.id = local.id;
					bound.target.version = local.version;
				}
			}
			entry.config = serde_json::to_value(c)?;
		}
		"tool" if crate::tool::legacy_config(&entry.config)?.is_some() => {
			let mut config = crate::tool::legacy_config(&entry.config)?.expect("legacy transport");
			if let ToolConfig::Agent { agent, .. } = &mut config {
				bind(agent, bindings);
			}
			entry.config = serde_json::to_value(config)?;
		}
		"tool" => {
			crate::configuration::validate_node_id(node)?;
			let mut c: crate::tool::providers::ToolDescriptor =
				serde_json::from_value(entry.config.clone())?;
			c.registry_node = node.into();
			if let Some(ToolConfig::Agent { agent, .. }) = &mut c.transport {
				bind(agent, bindings);
			}
			if let Some(lifecycle) = &mut c.lifecycle {
				for reference in [&mut lifecycle.poll, &mut lifecycle.cancel] {
					let mut local = reference.local();
					if bind(&mut local, bindings) {
						reference.registry_node = node.into();
						reference.id = local.id;
						reference.version = local.version;
					}
				}
			}
			entry.config = serde_json::to_value(c)?;
		}
		"bundle" => {
			crate::configuration::validate_node_id(node)?;
			let mut c: crate::registry::bindings::BundleConfig =
				serde_json::from_value(entry.config.clone())?;
			for reference in &mut c.members {
				let mut local = reference.local();
				if bind(&mut local, bindings) {
					reference.registry_node = node.into();
					reference.id = local.id;
					reference.version = local.version;
				}
			}
			entry.config = serde_json::to_value(c)?;
		}
		"cluster" => {
			let mut c: ClusterConfig = serde_json::from_value(entry.config.clone())?;
			bind(&mut c.coordinator, bindings);
			entry.config = serde_json::to_value(c)?;
		}
		_ => {}
	}
	entry.normalize_agent(node)?;
	Ok(())
}

pub fn installation_id(tenant: &str, package: &str) -> String {
	key(&(tenant, package))
}
pub fn manifest(version: &Version) -> Result<Package> {
	if format!(
		"sha256:{:x}",
		Sha256::digest(version.manifest_source.as_bytes())
	) != version.digest
	{
		return Err(conflict());
	}
	let package: Package =
		serde_json::from_str(&version.manifest_source).map_err(|_| conflict())?;
	if package.entity.kind != version.kind
		|| package.entity.version != version.version
		|| package.entity.installation.is_some()
	{
		return Err(conflict());
	}
	Ok(package)
}
fn conflict() -> Error {
	Error::Conflict("revision or immutable content changed".into())
}

#[cfg(test)]
mod tests;

pub fn prospective_installation(tenant: &str, package: &str) -> Installation {
	Installation {
		id: installation_id(tenant, package),
		tenant: tenant.into(),
		package_key: package.into(),
		latest_revision: 0,
		active_revision: None,
		activation_revision: 0,
	}
}
