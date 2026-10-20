//! A test offers the tools of its Agent's remote plugin servers from one listing made when it
//! starts. The session answers them only from fixtures, so no server is called after that.
use crate::{
	Result,
	plugin_tools::{agent_bindings, pin_for_run},
	ports::tools::PluginSessions,
};
use aidash_domain::{
	agent_plugin::{self, Component, PluginTools, Reason},
	registry::bindings::BindingSnapshot,
};
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::Arc,
};
use uuid::Uuid;

/// What the test pins from the remote servers of the Agent's plugins. Without a session port
/// nothing is listed and every server is reported unavailable, as a server that fails to answer is.
/// The listing's sessions end before this returns, with the pin or without it.
pub(super) async fn pin(
	sessions: Option<&Arc<dyn PluginSessions>>,
	snapshot: &BindingSnapshot,
) -> Result<PluginTools> {
	let Some(sessions) = sessions else {
		let agent = agent_bindings(snapshot)?;
		return Ok(agent_plugin::pin(
			&agent.agent_plugins,
			&agent.agent_plugin_settings,
			&BTreeMap::new(),
			&BTreeSet::new(),
		));
	};
	// The test session does not exist before admission commits, so the listing has its own key.
	let listing = Uuid::new_v4();
	let pinned = pin_for_run(sessions.as_ref(), listing, snapshot).await;
	sessions.close(listing).await;
	pinned
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
