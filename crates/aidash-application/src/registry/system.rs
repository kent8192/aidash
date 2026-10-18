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
		.chain([SKILL_ASSET_READ, TASK_ASSIGN])
}

/// Starts a generation inside Aidash only, so it is an ordinary built-in that
/// needs no Catalog Approval.
const TASK_ASSIGN: &str = "task_assign";

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

/// Writes every built-in definition into the scope's Tenant: the ordinary
/// built-ins, and the Sandboxed and research groups the Deployment offers,
/// which the Tenant approves before use. The caller owns one transaction.
/// Conflicting bytes fail.
pub async fn seed(
	scope: &mut dyn DefinitionWriter,
	validation: &DefinitionValidation,
	node: &str,
) -> Result<()> {
	let offered = packages::offered(validation, validation, node)?
		.into_iter()
		.flat_map(|group| group.operations.into_iter().chain([group.bundle]));
	for entry in entries(validation, node)?.into_iter().chain(offered) {
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

/// A definition the Deployment provides to every Tenant, which no Tenant changes:
/// its identifier carries a reserved prefix (ADR 0033).
pub fn is_deployment_definition(entry: &Entry) -> bool {
	entry.id.starts_with("aidash.") || entry.id.starts_with("core.")
}

pub fn reject_distribution(entry: &Entry) -> Result<()> {
	if is_deployment_definition(entry) {
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
			.is_ok_and(|descriptor| {
				// Node providers are declared only by the Node's packages.
				descriptor.provider.starts_with("core.")
					|| descriptor.provider.starts_with("research.")
			}) {
		return Err(crate::Error::Forbidden);
	}
	Ok(())
}

pub fn attributes(entry: &Entry) -> serde_json::Value {
	let deployment = is_deployment_definition(entry);
	json!({"origin":if deployment {"system"} else {"owner"},"immutable":deployment})
}

pub mod packages;
#[cfg(test)]
mod tests;
