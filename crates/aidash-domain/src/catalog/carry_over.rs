//! Consent-surface comparison for later Catalog versions (ADR 0128).
use crate::{
	Error, Result,
	decision::DeciderConfig,
	model::ModelConfig,
	registry::{
		AgentConfig, Entry,
		bindings::{AgentBindings, Narrowing, QualifiedRef, definition_references},
	},
	tool::{SecretOwner, ToolConfig, providers::ToolDescriptor},
};
use std::collections::BTreeSet;

fn host_changed(old: &str, new: &str) -> bool {
	let host = |endpoint: &str| {
		url::Url::parse(endpoint)
			.ok()
			.and_then(|url| url.host_str().map(str::to_owned))
	};
	match (host(old), host(new)) {
		(Some(old), Some(new)) => old != new,
		_ => true,
	}
}

/// A tool's transport and, for a provider descriptor, the descriptor itself.
fn tool(entry: &Entry) -> Result<(Option<ToolConfig>, Option<ToolDescriptor>)> {
	if let Some(config) = crate::tool::legacy_config(&entry.config)? {
		return Ok((Some(config), None));
	}
	let descriptor = serde_json::from_value::<ToolDescriptor>(entry.config.clone())?;
	Ok((descriptor.transport.clone(), Some(descriptor)))
}

/// Whether `new` lets a Binding or descriptor do anything `old` did not.
/// Removing or loosening a restriction widens it; tightening does not
/// (decision 3 of #873). Provider tool parameters choose an engine or model
/// rather than only bounding use, so any change to them counts.
fn narrowing_widened(old: &Narrowing, new: &Narrowing) -> bool {
	old.provider_tool != new.provider_tool
		|| !new.intersect(old).is_ok_and(|effective| effective == *new)
}

/// An empty member list binds the whole bundle.
fn members_widened(old: &[String], new: &[String]) -> bool {
	!old.is_empty() && (new.is_empty() || new.iter().any(|member| !old.contains(member)))
}

/// Where a Decider sends state. A System One Route names no host because it
/// reaches Jev only through TypeSafe, which is a destination of its own.
fn decider_destinations(config: &DeciderConfig) -> BTreeSet<(&str, &str)> {
	config
		.routes
		.iter()
		.flat_map(|route| {
			let contract = route.provider_contract.as_str();
			if route.hosts.is_empty() {
				vec![(contract, "")]
			} else {
				route
					.hosts
					.iter()
					.map(|host| (contract, host.as_str()))
					.collect()
			}
		})
		.collect()
}

/// A tool that only a person's own secret could authenticate now accepts a
/// shared Tenant Secret.
fn secret_owner_widened(old: SecretOwner, new: SecretOwner) -> bool {
	old == SecretOwner::Personal && new == SecretOwner::Any
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
			let a: ModelConfig = ModelConfig::from_stored(old.config.clone())?;
			let b: ModelConfig = ModelConfig::from_stored(new.config.clone())?;
			add(a.provider != b.provider, "provider");
			add(host_changed(&a.endpoint, &b.endpoint), "endpoint host");
			add(
				a.provider_credential != b.provider_credential,
				"provider credential",
			);
			add(a.plan_connection != b.plan_connection, "plan connection");
		}
		"decider" => {
			let a: DeciderConfig = serde_json::from_value(old.config.clone())?;
			let b: DeciderConfig = serde_json::from_value(new.config.clone())?;
			let destinations = decider_destinations(&a);
			add(
				decider_destinations(&b)
					.iter()
					.any(|destination| !destinations.contains(destination)),
				"route hosts",
			);
			// Like a model's, the credential decides whose account the state goes through.
			add(
				a.provider_credential != b.provider_credential,
				"provider credential",
			);
			add(
				b.state_retention.enabled
					&& (!a.state_retention.enabled
						|| b.state_retention.days > a.state_retention.days),
				"state retention",
			);
		}
		"tool" => {
			let (a, ad) = tool(old)?;
			let (b, bd) = tool(new)?;
			if let (Some(ad), Some(bd)) = (&ad, &bd) {
				add(
					ad.provider != bd.provider
						|| ad.operation != bd.operation
						|| ad.tier != bd.tier,
					"tool operation",
				);
				add(narrowing_widened(&ad.narrow, &bd.narrow), "restrictions");
				let lifecycle = |descriptor: &ToolDescriptor| {
					descriptor.lifecycle.as_ref().map(|lifecycle| {
						[&lifecycle.poll, &lifecycle.cancel]
							.map(|r| (r.registry_deployment.clone(), r.id.clone()))
					})
				};
				add(lifecycle(ad) != lifecycle(bd), "dependency");
			} else if ad.is_some() != bd.is_some() {
				add(true, "tool configuration");
			}
			// A Read-only declaration exempts each call from approval, so it
			// covers one exact operation. Keeping the declaration while the
			// operation changes declares another operation Read-only (decision 3).
			match (&a, &b) {
				(
					Some(ToolConfig::Http {
						endpoint: ae,
						secret: as_,
						replay: ar,
						secret_owner: ao,
					}),
					Some(ToolConfig::Http {
						endpoint: be,
						secret: bs,
						replay: br,
						secret_owner: bo,
					}),
				) => {
					add(host_changed(ae, be), "endpoint host");
					add(as_ != bs, "secret");
					add(secret_owner_widened(*ao, *bo), "secret owner");
					add(
						br == "read_only" && (ar != "read_only" || ae != be),
						"read-only operation",
					);
				}
				(
					Some(ToolConfig::Mcp {
						endpoint: ae,
						tool_name: at,
						secret: as_,
						scopes: ac,
						replay: ar,
						secret_owner: ao,
						..
					}),
					Some(ToolConfig::Mcp {
						endpoint: be,
						tool_name: bt,
						secret: bs,
						scopes: bc,
						replay: br,
						secret_owner: bo,
						..
					}),
				) => {
					add(host_changed(ae, be), "endpoint host");
					add(as_ != bs, "secret");
					add(secret_owner_widened(*ao, *bo), "secret owner");
					// Without declared scopes a Connection may request any scope, so
					// dropping the declaration widens it like adding a scope does.
					add(
						(ac.is_some() && bc.is_none())
							|| bc.as_ref().into_iter().flatten().any(|scope| {
								!ac.as_ref().is_some_and(|scopes| scopes.contains(scope))
							}),
						"scopes",
					);
					add(
						br == "read_only" && (ar != "read_only" || ae != be || at != bt),
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
			// Each Binding the Agent keeps, default ones included, must stay at
			// least as narrow: a dropped host, scope, limit or Decider
			// restriction, or a wider bundle member list, is new consent. A
			// Binding with no earlier counterpart is a dependency, reported above.
			let earlier = old.normalized_bindings(node)?;
			add(
				new.normalized_bindings(node)?.iter().any(|current| {
					let current = &current.binding;
					let mut counterparts = earlier
						.iter()
						.map(|item| &item.binding)
						.filter(|item| {
							item.kind == current.kind
								&& item.target.registry_deployment
									== current.target.registry_deployment
								&& item.target.id == current.target.id
						})
						.peekable();
					counterparts.peek().is_some()
						&& !counterparts.any(|item| {
							!narrowing_widened(&item.narrow, &current.narrow)
								&& !members_widened(&item.members, &current.members)
						})
				}),
				"binding restrictions",
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
