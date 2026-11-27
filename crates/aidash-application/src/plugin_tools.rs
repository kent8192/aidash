//! The tools a Run pins from the remote MCP servers of its Agent's plugins (ADR 0073). Listing is
//! network I/O, so the harness runs it once at the Run's first step, outside any transaction.
use crate::{
	Error, Result,
	ports::tools::{PluginServer, PluginSessions},
	tenant_secrets::CallSecret,
};
use aidash_domain::{
	agent_plugin::{
		self, ConnectionCause, Listing, McpServer, PluginName, PluginTools, RemoteServer,
		ServerName,
	},
	registry::bindings::{AgentBindings, BindingSnapshot},
	tool::providers::reserved_aliases,
};
use futures_util::{StreamExt, stream};
use std::{
	collections::{BTreeMap, BTreeSet},
	time::Duration,
};
use tokio::time::timeout;
use uuid::Uuid;

pub const SERVER_TIMEOUT: Duration = crate::mcp_listing::MCP_LIST_TIMEOUT;
/// Bounds how long a Run waits for one stdio server, allowing for Runner startup and
/// listing in its Execution Pod (ADR 0073).
pub const STDIO_SERVER_TIMEOUT: Duration = Duration::from_secs(330);
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

/// What each server of an Agent's plugins answered when it was listed.
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
/// its own deadline, and, where `sessions` runs them, every stdio server within
/// its own. A server that did not answer in time reports so.
pub async fn list_servers(
	sessions: &dyn PluginSessions,
	run: Uuid,
	agent: &AgentBindings,
) -> Listings {
	let (mut listed, stdio) = tokio::join!(
		list_remote(sessions, run, agent),
		list_stdio(sessions, run, agent)
	);
	listed.extend(stdio);
	listed
}

async fn list_stdio(sessions: &dyn PluginSessions, run: Uuid, agent: &AgentBindings) -> Listings {
	let Some(stdio) = sessions.stdio() else {
		return Listings::new();
	};
	let stdio = stdio.as_ref();
	let servers: Vec<_> = agent
		.agent_plugins
		.iter()
		.flat_map(|plugin| {
			plugin.servers.iter().filter_map(move |(name, server)| {
				let McpServer::Stdio(config) = server else {
					return None;
				};
				let server = PluginServer {
					plugin: plugin.name.clone(),
					digest: plugin.digest.clone(),
					server: name.clone(),
				};
				Some((server, config.clone()))
			})
		})
		.collect();
	let parallelism = servers.len().max(1);
	stream::iter(servers)
		.map(|(server, config)| async move {
			let listing =
				match timeout(STDIO_SERVER_TIMEOUT, stdio.list(run, &server, &config)).await {
					Ok(Ok(tools)) => Listing::Listed(tools),
					Ok(Err(Error::Invalid(cause))) => Listing::Unavailable(cause),
					Ok(Err(error)) => Listing::Unavailable(cause(&error)),
					Err(_) => Listing::Unavailable(format!(
						"{TIMEOUT}: stdio MCP server listing exceeded {} seconds",
						STDIO_SERVER_TIMEOUT.as_secs()
					)),
				};
			(key(&server), listing)
		})
		.buffer_unordered(parallelism)
		.collect()
		.await
}

async fn list_remote(sessions: &dyn PluginSessions, run: Uuid, agent: &AgentBindings) -> Listings {
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
	let parallelism = servers.len().max(1);
	stream::iter(servers)
		.map(|(server, config)| async move {
			let answer = list_one(sessions, run, &server, &config, None).await;
			let listing = remote_listing(answer, &config);
			(key(&server), listing)
		})
		.buffer_unordered(parallelism)
		.collect()
		.await
}

/// Reduce a remote listing to the same causes for Run starts and attach-time listings.
fn remote_listing(
	answer: Result<Vec<aidash_domain::agent_plugin::ListedTool>>,
	config: &RemoteServer,
) -> Listing {
	match answer {
		Ok(tools) => Listing::Listed(tools),
		Err(error) => connection_cause(&error, config.url.as_str())
			.map(Listing::Connection)
			.unwrap_or_else(|| Listing::Unavailable(cause(&error))),
	}
}

async fn list_one(
	sessions: &dyn PluginSessions,
	run: Uuid,
	server: &PluginServer,
	config: &RemoteServer,
	auth: Option<&CallSecret>,
) -> Result<Vec<aidash_domain::agent_plugin::ListedTool>> {
	timeout(SERVER_TIMEOUT, sessions.list(run, server, config, auth))
		.await
		.map_err(|_| crate::ports::tools::ListFailure::ListingTimeout)?
}

/// What one remote server lists for the person attaching or editing its Agent Plugin, so they can
/// declare Read-only tools (ADR 0073, amended 2026-11-16). `auth` is that person's OAuth
/// Connection or personal secret. The listing ends its session before returning.
pub async fn list_for_attach(
	sessions: &dyn PluginSessions,
	server: &PluginServer,
	config: &RemoteServer,
	auth: Option<&CallSecret>,
) -> Result<AttachListing> {
	let listing = Uuid::new_v4();
	let answer = list_one(sessions, listing, server, config, auth).await;
	sessions.close(listing).await;
	if matches!(&answer, Err(Error::Port(source)) if source.is::<crate::ports::tools::McpSessionMemoryShort>())
	{
		return answer.map(|tools| AttachListing::from(Listing::Listed(tools)));
	}
	Ok(remote_listing(answer, config).into())
}

/// One tool an attach-time listing returned: its name and the `inputSchema` digest a Read-only
/// declaration records, computed exactly as the Run-start pin computes it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct AttachTool {
	pub name: String,
	pub schema_digest: String,
}

/// The answer of an attach-time listing. When it is `unavailable`, Read-only cannot be declared
/// for the server; `reason` names a typed cause and any limit it hit:
/// `connection_required` or `connection_scope_insufficient` (no usable OAuth Connection),
/// `secret_unavailable` (the person has no personal secret of the named secret's name; the
/// shared Tenant Secret is never used here), `secret_rejected` (the server refused the person's
/// secret), or a transport cause such as `unreachable`, `initialize_failed`, `invalid_response`,
/// `forbidden` or `timeout`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AttachListing {
	Listed { tools: Vec<AttachTool> },
	Unavailable { reason: String },
}

impl From<Listing> for AttachListing {
	fn from(listing: Listing) -> Self {
		match listing {
			Listing::Listed(tools) => Self::Listed {
				tools: tools
					.iter()
					.map(|tool| AttachTool {
						name: tool.name.clone(),
						schema_digest: agent_plugin::schema_digest(&tool.input_schema),
					})
					.collect(),
			},
			Listing::Unavailable(reason) => Self::Unavailable { reason },
			Listing::Connection(cause) => Self::Unavailable {
				reason: cause.code().into(),
			},
		}
	}
}

impl AttachListing {
	/// The answer for a server whose credential could not be resolved: nothing was sent.
	pub fn unavailable(error: &Error) -> Self {
		Self::Unavailable {
			reason: if let Error::Invalid(code) = error
				&& code == aidash_domain::oauth_connections::CONNECTION_REQUIRED
			{
				code.clone()
			} else {
				cause(error)
			},
		}
	}
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
	let added = agent_plugin::pin(
		&agent.agent_plugins,
		&agent.agent_plugin_settings,
		&listed,
		&taken,
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
		// The server's named secret is missing, deleted, not for its host, or the definition
		// requires the person's own and they have none: nothing was sent.
		Error::Invalid(code)
			if code == aidash_domain::tenant_secrets::UNAVAILABLE
				|| crate::tenant_secrets::personal_error_value(error).is_some() =>
		{
			aidash_domain::tenant_secrets::UNAVAILABLE.into()
		}
		// The server refused the pasted secret: no connection would change that.
		Error::Invalid(message)
			if serde_json::from_str::<serde_json::Value>(message)
				.is_ok_and(|value| value["error"] == aidash_domain::tenant_secrets::REJECTED) =>
		{
			aidash_domain::tenant_secrets::REJECTED.into()
		}
		Error::Invalid(_) => "invalid_response".into(),
		Error::Port(source) if source.is::<crate::ports::tools::McpSessionMemoryShort>() => {
			format!(
				"{}: {source}",
				crate::ports::tools::McpSessionMemoryShort::CODE
			)
		}
		Error::Port(source) => source
			.downcast_ref::<crate::ports::tools::ListFailure>()
			.map_or_else(|| "unreachable".into(), ToString::to_string),
		_ => "unreachable".into(),
	}
}

#[cfg(test)]
mod tests;
