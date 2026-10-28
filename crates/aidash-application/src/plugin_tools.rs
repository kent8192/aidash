//! The tools a Run pins from the remote MCP servers of its Agent's plugins (ADR 0073). Listing is
//! network I/O, so the harness runs it once at the Run's first step, outside any transaction.
use crate::{
	Error, Result,
	ports::tools::{PluginServer, PluginSessions},
};
use aidash_domain::{
	agent_plugin::{
		self, ConnectionCause, Listing, McpServer, PluginName, PluginTools, ServerName,
	},
	registry::bindings::{AgentBindings, BindingSnapshot},
	tool::providers::reserved_aliases,
};
use futures_util::{StreamExt, stream};
use std::{
	collections::{BTreeMap, BTreeSet},
	time::Duration,
};
use tokio::time::{Instant, timeout, timeout_at};
use uuid::Uuid;

pub const SERVER_TIMEOUT: Duration = Duration::from_secs(15);
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
pub const CONCURRENCY: usize = 4;
/// The cause code of a server that did not answer in time.
const TIMEOUT: &str = "timeout";

/// The Agent definition a snapshot pins, as the Agent wrote it.
pub fn agent_bindings(snapshot: &BindingSnapshot) -> Result<AgentBindings> {
	let agent = snapshot
		.definitions
		.iter()
		.find(|definition| definition.identity == snapshot.agent)
		.ok_or_else(|| Error::Invalid("snapshot lacks its Agent definition".into()))?;
	Ok(serde_json::from_value(agent.definition.config.clone())?)
}

/// What each remote server of an Agent's plugins answered when it was listed.
pub type Listings = BTreeMap<(PluginName, ServerName), Listing>;

/// Lists every remote server of the Agent's plugins and decides which tools the Run pins. A server
/// that times out, is refused or fails is left out with a typed cause, never a URL or a body.
pub async fn pin_for_run(
	sessions: &dyn PluginSessions,
	run: Uuid,
	snapshot: &BindingSnapshot,
) -> Result<PluginTools> {
	let agent = agent_bindings(snapshot)?;
	let listed = list_servers(sessions, run, &agent).await;
	pin_listed(snapshot, &listed)
}

/// The network half of [`pin_for_run`]: every remote server of the Agent's plugins, listed within
/// the per-server and total deadlines. A server that did not answer in time reports so.
pub async fn list_servers(
	sessions: &dyn PluginSessions,
	run: Uuid,
	agent: &AgentBindings,
) -> Listings {
	let servers: Vec<_> = agent
		.agent_plugins
		.iter()
		.flat_map(|plugin| {
			plugin
				.servers
				.iter()
				.filter_map(|(name, server)| match server {
					McpServer::Remote(config) => Some((
						PluginServer {
							plugin: plugin.name.clone(),
							digest: plugin.digest.clone(),
							server: name.clone(),
						},
						config.clone(),
					)),
					McpServer::Stdio(_) => None,
				})
		})
		.collect();
	let keys: Vec<_> = servers.iter().map(|(server, _)| key(server)).collect();
	let deadline = Instant::now() + TOTAL_TIMEOUT;
	// The stream owns its servers and `key` is a function: with borrowing closures here, the
	// compiler cannot prove this future `Send` inside a route handler that awaits it.
	let mut answers = stream::iter(servers)
		.map(|(server, config)| async move {
			let listing =
				match timeout(SERVER_TIMEOUT, sessions.list(run, &server, &config, None)).await {
					Ok(Ok(tools)) => Listing::Listed(tools),
					Ok(Err(error)) => connection_cause(&error, config.url.as_str())
						.map(Listing::Connection)
						.unwrap_or_else(|| Listing::Unavailable(cause(&error))),
					Err(_) => Listing::Unavailable(TIMEOUT.into()),
				};
			(key(&server), listing)
		})
		.buffer_unordered(CONCURRENCY);
	let mut listed = BTreeMap::new();
	while let Ok(Some((server, listing))) = timeout_at(deadline, answers.next()).await {
		listed.insert(server, listing);
	}
	drop(answers);
	for server in keys {
		listed
			.entry(server)
			.or_insert_with(|| Listing::Unavailable(TIMEOUT.into()));
	}
	listed
}

/// The pure half of [`pin_for_run`]: decides the tools from what the servers listed. It reads
/// the snapshot's aliases and the Agent's plugin settings, and touches no network.
pub fn pin_listed(snapshot: &BindingSnapshot, listed: &Listings) -> Result<PluginTools> {
	let agent = agent_bindings(snapshot)?;
	let taken: BTreeSet<String> = reserved_aliases()
		.into_iter()
		.map(str::to_owned)
		.chain(
			snapshot
				.bindings
				.iter()
				.filter(|binding| binding.excluded_reason.is_none())
				.filter_map(|binding| binding.alias.clone()),
		)
		.collect();
	Ok(agent_plugin::pin(
		&agent.agent_plugins,
		&agent.agent_plugin_settings,
		listed,
		&taken,
	))
}

/// Consent admits tools only from formerly unconnected servers of this resource.
/// Existing pins, their digests and settings remain untouched.
pub async fn reconnect_for_run(
	sessions: &dyn PluginSessions,
	run: Uuid,
	snapshot: &BindingSnapshot,
	pinned: &PluginTools,
	resource: &str,
) -> Result<PluginTools> {
	use aidash_domain::agent_plugin::Component;
	let mut agent = agent_bindings(snapshot)?;
	let targets: BTreeSet<_> = pinned
		.findings
		.iter()
		.filter(|finding| {
			finding
				.connection
				.as_ref()
				.is_some_and(|cause| cause.resource() == resource)
		})
		.filter_map(|finding| {
			let Component::Server(server) = &finding.finding.component else {
				return None;
			};
			Some((finding.plugin.clone(), server.clone()))
		})
		.collect();
	for plugin in &mut agent.agent_plugins {
		plugin.servers.retain(|name, config| {
            targets.contains(&(plugin.name.clone(), name.to_string())) && matches!(config, McpServer::Remote(config) if aidash_domain::oauth_connections::canonical_resource(config.url.as_str()).ok().as_deref() == Some(resource))
        });
	}
	agent
		.agent_plugins
		.retain(|plugin| !plugin.servers.is_empty());
	let listed = list_servers(sessions, run, &agent).await;
	let taken: BTreeSet<String> = reserved_aliases()
		.into_iter()
		.map(str::to_owned)
		.chain(
			snapshot
				.bindings
				.iter()
				.filter(|binding| binding.excluded_reason.is_none())
				.filter_map(|binding| binding.alias.clone()),
		)
		.chain(
			pinned
				.servers
				.iter()
				.flat_map(|server| server.tools.iter().map(|tool| tool.alias.clone())),
		)
		.collect();
	let added = agent_plugin::pin_additional(
		&agent.agent_plugins,
		&agent.agent_plugin_settings,
		&listed,
		&taken,
		pinned.servers.iter().map(|server| server.tools.len()).sum(),
	);
	let mut result = pinned.clone();
	result.findings.retain(|finding| {
        !matches!(&finding.finding.component, Component::Server(server) if targets.contains(&(finding.plugin.clone(), server.clone())))
    });
	result.servers.extend(added.servers);
	result.findings.extend(added.findings);
	Ok(result)
}

fn key(server: &PluginServer) -> (PluginName, ServerName) {
	(server.plugin.clone(), server.server.clone())
}

/// Only transport-created, resource-matching challenges become retained connection causes.
fn connection_cause(error: &Error, endpoint: &str) -> Option<ConnectionCause> {
	let Error::Connection(cause) = error else {
		return None;
	};
	let resource = aidash_domain::oauth_connections::canonical_resource(endpoint).ok()?;
	(cause.resource() == resource).then_some(cause.clone())
}

fn cause(error: &Error) -> String {
	match error {
		Error::Forbidden => "forbidden".into(),
		Error::Invalid(_) => "invalid_response".into(),
		Error::Port(source) => source
			.downcast_ref::<crate::ports::tools::ListFailure>()
			.map_or_else(|| "unreachable".into(), ToString::to_string),
		_ => "unreachable".into(),
	}
}

#[cfg(test)]
mod tests;
