//! Importing an Agent Plugin: fetch it, keep the Tenant's copy and return the reference
//! an Agent definition records (ADR 0072).
use crate::{
	Error, Result,
	apps::registry::{
		repositories::agent_plugins,
		serializers::agent_plugin_import::{
			AgentPluginImportResult, ImportedAgentPlugin, PluginImportRequest,
		},
	},
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use aidash_application::registry::agent_plugin_import::{AgentPluginImport, PluginImport};
use aidash_domain::agent_plugin::{AgentPluginRef, LoadedPlugin, PluginFile};

/// Needs `registry.create` on an Agent, because a copy only serves Agent definitions. The
/// check comes before any network access, and the copy is stored in a second transaction that
/// decides again, so the fetch holds no database connection.
pub async fn import(
	runtime: &Federation,
	source: &AgentPluginImport,
	actor: Actor,
	request: PluginImportRequest,
) -> Result<AgentPluginImportResult> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&runtime.store, &identity).await?;
	let result = require_agent_create(&mut access).await;
	access.finish(result).await?;
	match source.import(request).await? {
		PluginImport::Choose(roots) => Ok(AgentPluginImportResult {
			plugins: roots,
			selected: None,
		}),
		PluginImport::Loaded {
			source,
			root,
			denied,
			plugin,
			files,
		} => {
			let mut access = Access::begin(&runtime.store, &identity).await?;
			let result = store(
				&mut access,
				&runtime.registry.validation(),
				&source,
				&plugin,
				&files,
				denied,
			)
			.await;
			let selected = access.finish(result).await?;
			Ok(AgentPluginImportResult {
				plugins: vec![root],
				selected: Some(selected),
			})
		}
	}
}

async fn require_agent_create(access: &mut Access) -> Result<()> {
	let resource = access.resource(
		"registry",
		format!(
			"agent/{}",
			aidash_domain::identity::tenant_administration::COLLECTION
		),
		serde_json::json!({"kind": "agent"}),
	);
	access.require(&resource, "registry.create").await
}

/// Keeps the copy in the caller's Tenant and builds the reference from the stored row, so that
/// an import after a later change to how plugins load still matches definitions registered
/// earlier.
async fn store(
	access: &mut Access,
	validation: &aidash_application::registry::DefinitionValidation,
	source: &str,
	plugin: &LoadedPlugin,
	files: &[PluginFile],
	denied: Vec<String>,
) -> Result<ImportedAgentPlugin> {
	require_agent_create(access).await?;
	validation.validate_agent_plugin(&AgentPluginRef::of(plugin, source))?;
	let stored = agent_plugins::insert(&mut **access.tx, source, plugin, files, &denied).await?;
	let reference = AgentPluginRef::of(&stored.plugin, &stored.source);
	reference.validate()?;
	Ok(ImportedAgentPlugin { reference, denied })
}
