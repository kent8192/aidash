//! Node declarations are immutable catalog documents, independent of tenant grants.
use crate::{Result, ports::registry::DefinitionWriter, registry::DefinitionValidation};
use aidash_domain::{
	registry::{
		Entry,
		bindings::{DEFAULT_TOOLS, EXPOSURE_TOOLS, REQUIRED_TOOLS, SKILL_ASSET_READ},
	},
	tool::providers::core_descriptor,
};
use serde_json::json;

/// Every system builtin operation of either Exposure policy, in seeding order.
pub fn operations() -> impl Iterator<Item = &'static str> {
	REQUIRED_TOOLS
		.iter()
		.chain(DEFAULT_TOOLS)
		.chain(EXPOSURE_TOOLS)
		.copied()
		.chain([SKILL_ASSET_READ])
}

pub fn entries(validation: &DefinitionValidation, node: &str) -> Result<Vec<Entry>> {
	let specifications = validation.node_specifications();
	operations()
		.map(|operation| {
			let spec = specifications.get(operation).ok_or_else(|| {
				crate::Error::Invalid(format!(
					"system provider has no declaration for {operation}"
				))
			})?;
			Ok(Entry {
				binding_normalization: None,
				installation: None,
				id: format!("aidash.{operation}"),
				version: "1.0.0".into(),
				kind: "tool".into(),
				name: std::collections::BTreeMap::from([("en".into(), operation.to_string())]),
				description: std::collections::BTreeMap::from([(
					"en".into(),
					spec.description.clone(),
				)]),
				capabilities: vec![operation.to_string()],
				tags: vec!["system".into()],
				languages: vec![],
				skills: vec![],
				schema: spec.parameters.clone(),
				config: serde_json::to_value(core_descriptor(node, operation)?)?,
			})
		})
		.collect()
}

/// The caller is Node composition and owns one transaction. Conflicting bytes fail.
pub async fn seed(
	scope: &mut dyn DefinitionWriter,
	validation: &DefinitionValidation,
	node: &str,
) -> Result<()> {
	for entry in entries(validation, node)? {
		aidash_domain::registry::rules::validate_metadata(&entry, false)?;
		// Restart may precede recovery of a prepared transaction. Even an
		// INSERT that does nothing takes Registry's statement-level write guard.
		match scope.definition(&entry.id, &entry.version).await {
			Ok(saved) if saved == entry => continue,
			Ok(_) => {
				return Err(crate::Error::Conflict(
					"system declaration bytes changed".into(),
				));
			}
			Err(crate::Error::NotFound(_)) => {}
			Err(error) => return Err(error),
		}
		scope.insert_definition(&entry).await?;
	}
	Ok(())
}

pub fn is_builtin(entry: &Entry) -> bool {
	entry.kind == "tool"
		&& serde_json::from_value::<aidash_domain::tool::providers::ToolDescriptor>(
			entry.config.clone(),
		)
		.is_ok_and(|descriptor| {
			descriptor.tier == aidash_domain::tool::providers::ToolTier::Builtin
		})
}

pub fn builtin_reference(reference: &aidash_domain::registry::EntityRef) -> bool {
	reference.version == "1.0.0"
		&& reference
			.id
			.strip_prefix("aidash.")
			.is_some_and(|operation| operations().any(|builtin| builtin == operation))
}

pub fn reject_distribution(entry: &Entry) -> Result<()> {
	if is_builtin(entry) {
		return Err(crate::Error::Invalid("system builtin declarations cannot be published, installed or mutated through Marketplace".into()));
	}
	Ok(())
}

pub fn reject_owner_definition(entry: &Entry) -> Result<()> {
	if entry.id.starts_with("aidash.")
		|| entry.id.starts_with("core.")
		|| entry.kind == "tool"
			&& serde_json::from_value::<aidash_domain::tool::providers::ToolDescriptor>(
				entry.config.clone(),
			)
			.is_ok_and(|descriptor| descriptor.provider.starts_with("core."))
	{
		return Err(crate::Error::Forbidden);
	}
	Ok(())
}

pub fn attributes(entry: &Entry) -> serde_json::Value {
	json!({"origin":if is_builtin(entry) {"system"} else {"owner"},"immutable":is_builtin(entry)})
}

pub mod packages;
#[cfg(test)]
mod tests;
