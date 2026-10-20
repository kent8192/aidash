//! A test offers the tools of its Agent's remote plugin servers from one listing made before it is
//! admitted. The session answers them only from fixtures, so no server is called after that.
use crate::{
	plugin_tools::{Listings, list_servers},
	ports::tools::PluginSessions,
};
use aidash_domain::{
	agent_plugin::{Component, PluginTools, Reason},
	registry::{bindings::AgentBindings, workbench::Draft},
};
use std::sync::Arc;
use uuid::Uuid;

/// The Agent definition a draft holds, when it parses and takes in a plugin with a remote server.
/// A draft that does not parse is for admission to refuse.
pub(super) fn remote_plugins(draft: &Draft) -> Option<AgentBindings> {
	let agent: AgentBindings = serde_json::from_value(draft.entry.get("config")?.clone()).ok()?;
	agent
		.agent_plugins
		.iter()
		.any(|plugin| {
			plugin
				.servers
				.values()
				.any(|server| matches!(server, aidash_domain::agent_plugin::McpServer::Remote(_)))
		})
		.then_some(agent)
}

/// What the remote servers of the draft's plugins list. Listing waits on other hosts for up to
/// 30 s, so admission runs it before it takes the draft and Tenant-limits locks and hands the
/// answers in. The listing's sessions end before this returns.
pub(super) async fn list(sessions: &Arc<dyn PluginSessions>, agent: &AgentBindings) -> Listings {
	// The test session does not exist before admission commits, so the listing has its own key.
	let listing = Uuid::new_v4();
	let listed = list_servers(sessions.as_ref(), listing, agent).await;
	sessions.close(listing).await;
	listed
}

/// One line telling the model which plugin servers contribute no tools to this test.
pub(super) fn note(pinned: &PluginTools) -> Option<String> {
	let unavailable: Vec<String> = pinned
		.findings
		.iter()
		.filter(|entry| entry.finding.reason == Reason::ServerUnavailable)
		.filter_map(|entry| match &entry.finding.component {
			Component::Server(server) => Some(format!("{}/{server}", entry.plugin.as_str())),
			_ => None,
		})
		.collect();
	(!unavailable.is_empty()).then(|| {
		format!(
			"\nSandbox: the tools of the plugin servers {} are unavailable in this test.\n",
			unavailable.join(", ")
		)
	})
}
