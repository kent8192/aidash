//! Consent-surface comparison for later Catalog versions (ADR 0128).
use crate::{
	Error, Result,
	decision::DeciderConfig,
	model::ModelConfig,
	registry::{
		AgentConfig, Entry,
		bindings::{AgentBindings, QualifiedRef, definition_references},
	},
	tool::{ToolConfig, providers::ToolDescriptor},
};
use std::collections::BTreeSet;

fn host(endpoint: &str) -> Result<String> {
	url::Url::parse(endpoint)
		.ok()
		.and_then(|url| url.host_str().map(str::to_owned))
		.ok_or_else(|| Error::Invalid("invalid consent endpoint host".into()))
}

fn tool(entry: &Entry) -> Result<Option<ToolConfig>> {
	if let Some(config) = crate::tool::legacy_config(&entry.config)? {
		return Ok(Some(config));
	}
	Ok(serde_json::from_value::<ToolDescriptor>(entry.config.clone())?.transport)
}

/// Compare two versions of one definition. An empty result permits carry-over;
/// dependency approvals are checked separately on each Node.
pub fn compare(old: &Entry, new: &Entry) -> Result<Vec<String>> {
	if old.id != new.id || old.kind != new.kind {
		return Err(Error::Invalid(
			"consent comparison requires the same definition".into(),
		));
	}
	let mut changes = Vec::new();
	let mut add = |condition: bool, name: &str| {
		if condition {
			changes.push(name.to_owned());
		}
	};
	match old.kind.as_str() {
		"model" => {
			let a: ModelConfig = serde_json::from_value(old.config.clone())?;
			let b: ModelConfig = serde_json::from_value(new.config.clone())?;
			add(a.provider != b.provider, "provider");
			add(host(&a.endpoint)? != host(&b.endpoint)?, "endpoint host");
			add(
				a.provider_credential != b.provider_credential,
				"provider credential",
			);
			add(a.plan_connection != b.plan_connection, "plan connection");
		}
		"decider" => {
			let a: DeciderConfig = serde_json::from_value(old.config.clone())?;
			let b: DeciderConfig = serde_json::from_value(new.config.clone())?;
			let hosts: BTreeSet<_> = a.routes.iter().flat_map(|r| &r.hosts).collect();
			add(
				b.routes
					.iter()
					.flat_map(|r| &r.hosts)
					.any(|h| !hosts.contains(h)),
				"route hosts",
			);
			add(
				b.state_retention.enabled
					&& (!a.state_retention.enabled
						|| b.state_retention.days > a.state_retention.days),
				"state retention",
			);
		}
		"tool" => {
			let a = tool(old)?;
			let b = tool(new)?;
			match (&a, &b) {
				(
					Some(ToolConfig::Http {
						endpoint: ae,
						secret: as_,
						replay: ar,
						..
					}),
					Some(ToolConfig::Http {
						endpoint: be,
						secret: bs,
						replay: br,
						..
					}),
				) => {
					add(host(ae)? != host(be)?, "endpoint host");
					add(as_ != bs, "secret");
					add(
						ar != "read_only" && br == "read_only",
						"read-only operation",
					);
				}
				(
					Some(ToolConfig::Mcp {
						endpoint: ae,
						secret: as_,
						scopes: ac,
						replay: ar,
						..
					}),
					Some(ToolConfig::Mcp {
						endpoint: be,
						secret: bs,
						scopes: bc,
						replay: br,
						..
					}),
				) => {
					add(host(ae)? != host(be)?, "endpoint host");
					add(as_ != bs, "secret");
					add(
						bc.as_ref()
							.into_iter()
							.flatten()
							.any(|scope| !ac.as_ref().is_some_and(|scopes| scopes.contains(scope))),
						"scopes",
					);
					add(
						ar != "read_only" && br == "read_only",
						"read-only operation",
					);
				}
				_ => add(old.config != new.config, "tool configuration"),
			}
		}
		"agent" => {
			let a: AgentBindings = serde_json::from_value(old.config.clone())?;
			let b: AgentBindings = serde_json::from_value(new.config.clone())?;
			let node = old
				.binding_normalization
				.as_ref()
				.or(new.binding_normalization.as_ref())
				.map_or("aidash://local", |n| n.registry_deployment.as_str());
			let refs = |entry: &Entry| -> Result<BTreeSet<(String, String, String)>> {
				let identity = QualifiedRef {
					registry_deployment: node.to_owned(),
					id: entry.id.clone(),
					version: entry.version.clone(),
				};
				Ok(definition_references(&identity, entry)?
					.into_iter()
					.map(|(r, k)| (r.registry_deployment, r.id, k))
					.collect())
			};
			let previous = refs(old)?;
			add(
				refs(new)?.iter().any(|r| !previous.contains(r)),
				"dependency",
			);
			let ac = AgentConfig::from_definition(a.clone());
			let bc = AgentConfig::from_definition(b.clone());
			let ca = &ac.core_capabilities;
			let cb = &bc.core_capabilities;
			add(
				(cb.outbound && !ca.outbound)
					|| (cb.files && !ca.files)
					|| (cb.shell && !ca.shell)
					|| (cb.python && !ca.python)
					|| (cb.patch && !ca.patch)
					|| (cb.skills && !ca.skills)
					|| (cb.sharing && !ca.sharing),
				"core capabilities",
			);
			add(
				[
					(ac.allow_task_creation, bc.allow_task_creation),
					(ac.allow_task_delegation, bc.allow_task_delegation),
					(ac.allow_memory_write, bc.allow_memory_write),
					(ac.allow_workspace_retrieval, bc.allow_workspace_retrieval),
					(
						ac.allow_cross_conversation_memory,
						bc.allow_cross_conversation_memory,
					),
				]
				.iter()
				.any(|(x, y)| *y == Some(true) && *x != Some(true)),
				"allow flags",
			);
			add(
				b.agent_plugins
					.iter()
					.any(|plugin| !a.agent_plugins.contains(plugin)),
				"agent plugins",
			);
			let mut plugin_settings_widened = false;
			for plugin in &b.agent_plugins {
				for server in plugin.servers.keys() {
					let old_settings = a
						.agent_plugin_settings
						.get(&plugin.name)
						.and_then(|servers| servers.get(server))
						.cloned()
						.unwrap_or_default();
					let new_settings = b
						.agent_plugin_settings
						.get(&plugin.name)
						.and_then(|servers| servers.get(server))
						.cloned()
						.unwrap_or_default();
					if let Some(old_tools) = &old_settings.tools {
						plugin_settings_widened |= new_settings
							.tools
							.as_ref()
							.is_none_or(|new_tools| !new_tools.is_subset(old_tools));
					}
					plugin_settings_widened |= new_settings
						.read_only
						.iter()
						.any(|item| !old_settings.read_only.contains(item));
				}
			}
			add(plugin_settings_widened, "agent plugin settings");
			add(
				a.remove_default.iter().collect::<BTreeSet<_>>()
					!= b.remove_default.iter().collect::<BTreeSet<_>>(),
				"remove default",
			);
		}
		_ => add(old.config != new.config, "configuration"),
	}
	Ok(changes)
}

/// A candidate approval on one Node. Disabled or non-carrying rows cannot be sources.
pub struct CarryCandidate<'a> {
	pub version: &'a str,
	pub enabled: bool,
	pub carry_over: bool,
}

/// Pick the highest eligible SemVer strictly below the registered version.
pub fn source_version<'a>(
	target: &str,
	candidates: &[CarryCandidate<'a>],
) -> Result<Option<&'a str>> {
	let target = semver::Version::parse(target)
		.map_err(|_| Error::Invalid("invalid target version".into()))?;
	let mut best = None;
	for candidate in candidates
		.iter()
		.filter(|candidate| candidate.enabled && candidate.carry_over)
	{
		let version = semver::Version::parse(candidate.version)
			.map_err(|_| Error::Invalid("invalid approval source version".into()))?;
		if version.cmp_precedence(&target).is_lt()
			&& best.as_ref().is_none_or(|(previous, original)| {
				version.cmp_precedence(previous).is_gt()
					|| (version.cmp_precedence(previous).is_eq() && candidate.version > *original)
			}) {
			best = Some((version, candidate.version));
		}
	}
	Ok(best.map(|(_, original)| original))
}

#[cfg(test)]
mod tests;
