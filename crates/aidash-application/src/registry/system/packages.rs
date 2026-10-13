//! Operator declarations for the host and research package groups. No executable payloads.
use super::*;
use crate::{Error, ports::bindings::ProviderCatalog};
use aidash_domain::{
	identity::Principal,
	registry::bindings::{BundleConfig, QualifiedRef},
	tool::providers::{ToolDescriptor, ToolTier, research_descriptor},
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
/// Research provider packages, one group per Node provider. Every group's
/// operations share the research contract, so a Run binding the fixture group
/// behaves exactly as one binding the Brave and page groups.
pub const RESEARCH_GROUPS: &[(&str, &str)] = &[
	("research_brave", "research.brave@1"),
	("research_page", "research.page@1"),
	("research_fixture", "research.fixture@1"),
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
						binding_normalization: None,
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
			Ok(PackageGroup {
				name: (*name).into(),
				bundle: bundle_entry(node, name, &operations)?,
				operations,
			})
		})
		.chain(RESEARCH_GROUPS.iter().map(|(name, provider)| {
			let (_, operations, _) = aidash_domain::tool::providers::RESEARCH_PROVIDERS
				.iter()
				.find(|(candidate, _, _)| candidate == provider)
				.ok_or_else(|| Error::Invalid(format!("unknown research provider {provider}")))?;
			let stem = provider.trim_end_matches("@1");
			let operations = operations
				.iter()
				.map(|operation| {
					let descriptor = research_descriptor(node, provider, operation)?;
					let research = aidash_domain::research::Operation::parse(operation)
						.expect("declared research operation");
					let spec = crate::research::specification(research);
					Ok(Entry {
						binding_normalization: None,
						installation: None,
						id: format!("aidash.{stem}.{operation}"),
						version: "1.0.0".into(),
						kind: "tool".into(),
						name: BTreeMap::from([("en".into(), spec.name.clone())]),
						description: BTreeMap::from([("en".into(), spec.description)]),
						capabilities: vec![format!("research.{operation}")],
						tags: vec!["operator".into(), "research".into()],
						languages: vec![],
						skills: vec![],
						schema: spec.parameters,
						config: serde_json::to_value(descriptor)?,
					})
				})
				.collect::<Result<Vec<_>>>()?;
			Ok(PackageGroup {
				name: (*name).into(),
				bundle: bundle_entry(node, name, &operations)?,
				operations,
			})
		}))
		.collect()
}

fn bundle_entry(node: &str, name: &str, operations: &[Entry]) -> Result<Entry> {
	Ok(Entry {
		binding_normalization: None,
		installation: None,
		id: format!("aidash.bundle.{name}"),
		version: "1.0.0".into(),
		kind: "bundle".into(),
		name: BTreeMap::from([("en".into(), format!("Aidash {name}"))]),
		description: BTreeMap::from([("en".into(), format!("Node-provided {name} operations"))]),
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
	})
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
