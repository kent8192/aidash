//! The tools a Run pins from the remote MCP servers of its Agent's plugins (ADR 0073). Listing is
//! network I/O, so the harness runs it once at the Run's first step, outside any transaction.
use crate::{
	Error, Result,
	ports::tools::{PluginServer, PluginSessions},
};
use aidash_domain::{
	agent_plugin::{self, Listing, McpServer, PluginTools},
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

pub const SERVER_TIMEOUT: Duration = Duration::from_secs(10);
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

/// Lists every remote server of the Agent's plugins and decides which tools the Run pins. A server
/// that times out, is refused or fails is left out with a typed cause, never a URL or a body.
pub async fn pin_for_run(
	sessions: &dyn PluginSessions,
	run: Uuid,
	snapshot: &BindingSnapshot,
) -> Result<PluginTools> {
	let agent = agent_bindings(snapshot)?;
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
						config,
					)),
					McpServer::Stdio(_) => None,
				})
		})
		.collect();
	let key = |server: &PluginServer| (server.plugin.clone(), server.server.clone());
	let deadline = Instant::now() + TOTAL_TIMEOUT;
	let mut answers = stream::iter(&servers)
		.map(|(server, config)| async move {
			let listing = match timeout(SERVER_TIMEOUT, sessions.list(run, server, config)).await {
				Ok(Ok(tools)) => Listing::Listed(tools),
				Ok(Err(error)) => Listing::Unavailable(cause(&error)),
				Err(_) => Listing::Unavailable(TIMEOUT.into()),
			};
			(key(server), listing)
		})
		.buffer_unordered(CONCURRENCY);
	let mut listed = BTreeMap::new();
	while let Ok(Some((server, listing))) = timeout_at(deadline, answers.next()).await {
		listed.insert(server, listing);
	}
	drop(answers);
	for (server, _) in &servers {
		listed
			.entry(key(server))
			.or_insert_with(|| Listing::Unavailable(TIMEOUT.into()));
	}
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
		&listed,
		&taken,
	))
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
