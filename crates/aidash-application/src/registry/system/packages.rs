//! The Sandboxed and research groups a Deployment offers every Tenant as
//! built-in definitions that each Tenant approves before use (ADR 0034). No
//! executable payloads.
use super::*;
use crate::{Error, ports::bindings::ProviderCatalog};
use aidash_domain::{
	registry::bindings::{BundleConfig, QualifiedRef},
	tool::providers::{ToolDescriptor, ToolTier, research_descriptor},
};
use std::collections::BTreeMap;

pub const SANDBOXED_GROUPS: &[(&str, &[&str])] = &[
	("shell", &["shell", "shell_poll", "shell_cancel"]),
	(
		"python",
		&["code_interpreter", "python_poll", "python_cancel"],
	),
	("outbound_get", &["outbound_get"]),
	("apply_patch", &["apply_patch"]),
	("file_share", &["file_share"]),
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
					registry_deployment: descriptor.registry_deployment.clone(),
					id: entry.id.clone(),
					version: entry.version.clone(),
				},
			)?;
			providers.implementation(&descriptor)?;
		}
		Ok(())
	}
}
/// The groups whose provider the Deployment runs. A Deployment without a
/// runner offers no Sandboxed group (ADR 0034).
pub fn offered(
	validation: &DefinitionValidation,
	providers: &dyn ProviderCatalog,
	node: &str,
) -> Result<Vec<PackageGroup>> {
	Ok(declarations(validation, node)?
		.into_iter()
		.filter(|group| group.verify_provider(providers).is_ok())
		.collect())
}

/// Every Sandboxed and research group, whether or not the Deployment runs it.
pub fn declarations(validation: &DefinitionValidation, node: &str) -> Result<Vec<PackageGroup>> {
	aidash_domain::nodes::DeploymentId::parse(node)?;
	let specs = validation.node_specifications();
	SANDBOXED_GROUPS
		.iter()
		.map(|(name, operations)| {
			let operations = operations
				.iter()
				.map(|operation| {
					let spec = specs.get(*operation).ok_or_else(|| {
						Error::Invalid(format!("provider has no specification for {operation}"))
					})?;
					let descriptor = core_descriptor(node, operation)?;
					if descriptor.tier != ToolTier::Sandboxed {
						return Err(Error::Invalid(
							"a Sandboxed group contains a builtin".into(),
						));
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
						tags: vec!["sandboxed".into()],
						languages: vec![],
						skills: vec![],
						schema: spec.parameters.clone(),
						config: serde_json::to_value(descriptor)?,
					})
				})
				.collect::<Result<Vec<_>>>()?;
			Ok(PackageGroup {
				name: (*name).into(),
				bundle: bundle_entry(node, name, "sandboxed", &operations)?,
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
						tags: vec!["research".into()],
						languages: vec![],
						skills: vec![],
						schema: spec.parameters,
						config: serde_json::to_value(descriptor)?,
					})
				})
				.collect::<Result<Vec<_>>>()?;
			Ok(PackageGroup {
				name: (*name).into(),
				bundle: bundle_entry(node, name, "research", &operations)?,
				operations,
			})
		}))
        .chain(aidash_domain::tool::provider_tools::TOOLS.iter().map(|tool| {
            let descriptor = aidash_domain::tool::providers::ToolDescriptor {
                registry_deployment: node.into(), provider: tool.id.into(), operation: tool.alias.into(),
                default_alias: tool.alias.into(), tier: ToolTier::Provider, narrow: Default::default(), transport: None, lifecycle: None,
            };
            let entry = Entry {
                binding_normalization: None, installation: None,
                id: tool.id.trim_end_matches("@1").into(), version: "1.0.0".into(), kind: "tool".into(),
                name: BTreeMap::from([("en".into(),tool.tool_type.into())]),
                description: BTreeMap::from([("en".into(),"Provider tool activity is not Research evidence. Parameters and spending caps are pinned by the Agent Binding.".into())]),
                capabilities: vec![tool.alias.into()], tags: vec!["provider".into()], languages: vec![], skills: vec![],
                schema: tool.binding_schema(), config: serde_json::to_value(descriptor)?,
            };
            Ok(PackageGroup { name: tool.alias.into(), bundle: bundle_entry(node,tool.alias,"provider",std::slice::from_ref(&entry))?, operations:vec![entry] })
        }))
        .collect()
}

fn bundle_entry(node: &str, name: &str, tag: &str, operations: &[Entry]) -> Result<Entry> {
	Ok(Entry {
		binding_normalization: None,
		installation: None,
		id: format!("aidash.bundle.{name}"),
		version: if matches!(name, "research_page" | "research_fixture") {
			"1.1.0"
		} else {
			"1.0.0"
		}
		.into(),
		kind: "bundle".into(),
		name: BTreeMap::from([("en".into(), format!("Aidash {name}"))]),
		description: BTreeMap::from([("en".into(), format!("Node-provided {name} operations"))]),
		capabilities: operations
			.iter()
			.flat_map(|entry| entry.capabilities.clone())
			.collect(),
		tags: vec![tag.into()],
		languages: vec![],
		skills: vec![],
		schema: json!({}),
		config: serde_json::to_value(BundleConfig {
			members: operations
				.iter()
				.map(|entry| QualifiedRef {
					registry_deployment: node.into(),
					id: entry.id.clone(),
					version: entry.version.clone(),
				})
				.collect(),
		})?,
	})
}

#[cfg(test)]
mod tests;
