//! Operator declarations for the six host package groups. No executable payloads.
use super::*;
use crate::{Error, ports::bindings::ProviderCatalog};
use aidash_domain::{
	identity::Principal,
	registry::bindings::{BundleConfig, QualifiedRef},
	tool::providers::{ToolDescriptor, ToolTier},
};
use std::collections::{BTreeMap, BTreeSet};

pub const HOST_GROUPS: &[(&str, &[&str])] = &[
	("shell", &["shell", "shell_poll", "shell_cancel"]),
	(
		"python",
		&[
			"code_interpreter",
			"python_install",
			"python_poll",
			"python_cancel",
		],
	),
	("outbound_get", &["outbound_get"]),
	("apply_patch", &["apply_patch"]),
	("file_share", &["file_share"]),
	("task_assign", &["task_assign"]),
];
#[derive(Clone)]
pub struct PackageGroup {
	pub name: String,
	pub bundle: Entry,
	pub operations: Vec<Entry>,
}
impl PackageGroup {
	pub fn verify_provider(&self, providers: &dyn ProviderCatalog) -> Result<()> {
		for entry in &self.operations {
			let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone())?;
			providers.contract(
				&descriptor,
				&QualifiedRef {
					registry_node: descriptor.registry_node.clone(),
					id: entry.id.clone(),
					version: entry.version.clone(),
				},
			)?;
			providers.implementation(&descriptor)?;
		}
		Ok(())
	}
}
/// Authority comes from authentication, never the author's display name.
pub fn declarations(
	principal: &Principal,
	validation: &DefinitionValidation,
	node: &str,
) -> Result<Vec<PackageGroup>> {
	crate::authorization::require_operator(principal)?;
	aidash_domain::configuration::validate_node_id(node)?;
	let specs = validation.node_specifications();
	HOST_GROUPS
		.iter()
		.map(|(name, operations)| {
			let operations = operations
				.iter()
				.map(|operation| {
					let spec = specs.get(*operation).ok_or_else(|| {
						Error::Invalid(format!("provider has no specification for {operation}"))
					})?;
					let descriptor = core_descriptor(node, operation)?;
					if descriptor.tier != ToolTier::Host {
						return Err(Error::Invalid("host package contains a builtin".into()));
					}
					Ok(Entry {
						installation: None,
						id: format!("aidash.{operation}"),
						version: "1.0.0".into(),
						kind: "tool".into(),
						name: BTreeMap::from([("en".into(), operation.to_string())]),
						description: BTreeMap::from([("en".into(), spec.description.clone())]),
						capabilities: vec![operation.to_string()],
						tags: vec!["operator".into()],
						languages: vec![],
						skills: vec![],
						schema: spec.parameters.clone(),
						config: serde_json::to_value(descriptor)?,
					})
				})
				.collect::<Result<Vec<_>>>()?;
			let bundle = Entry {
				installation: None,
				id: format!("aidash.bundle.{name}"),
				version: "1.0.0".into(),
				kind: "bundle".into(),
				name: BTreeMap::from([("en".into(), format!("Aidash {name}"))]),
				description: BTreeMap::from([(
					"en".into(),
					format!("Node-provided {name} operations"),
				)]),
				capabilities: operations
					.iter()
					.flat_map(|entry| entry.capabilities.clone())
					.collect(),
				tags: vec!["operator".into()],
				languages: vec![],
				skills: vec![],
				schema: json!({}),
				config: serde_json::to_value(BundleConfig {
					members: operations
						.iter()
						.map(|entry| QualifiedRef {
							registry_node: node.into(),
							id: entry.id.clone(),
							version: entry.version.clone(),
						})
						.collect(),
				})?,
			};
			Ok(PackageGroup {
				name: (*name).into(),
				bundle,
				operations,
			})
		})
		.collect()
}

pub struct ProvisioningPlan {
	pub pending: Vec<PackageGroup>,
	pub unavailable: BTreeMap<String, String>,
}
/// Called only for a newly created tenant. The returned entries are candidates
/// for pending revisions; provider loss never selects an older package or makes
/// an approved installation. Existing tenants have no startup adoption path.
pub fn configured_defaults(
	principal: &Principal,
	validation: &DefinitionValidation,
	providers: &dyn ProviderCatalog,
	node: &str,
	selected: &[String],
) -> Result<ProvisioningPlan> {
	let groups = declarations(principal, validation, node)?;
	let mut seen = BTreeSet::new();
	let mut result = ProvisioningPlan {
		pending: vec![],
		unavailable: BTreeMap::new(),
	};
	for name in selected {
		if !seen.insert(name) {
			return Err(Error::Invalid("duplicate default host package".into()));
		}
		let group = groups
			.iter()
			.find(|group| &group.name == name)
			.ok_or_else(|| Error::Invalid(format!("unknown default host package: {name}")))?;
		match group.verify_provider(providers) {
			Ok(()) => result.pending.push(group.clone()),
			Err(error) => {
				result.unavailable.insert(name.clone(), error.to_string());
			}
		}
	}
	Ok(result)
}

#[cfg(test)]
mod tests;
