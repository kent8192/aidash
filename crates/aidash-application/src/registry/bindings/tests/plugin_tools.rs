use super::*;
use crate::{
	plugin_tools::pin_for_run,
	test_support::{FakeSessions, Listed, listed_tool, plugin_reference, run_of, stored_plugin},
};
use aidash_domain::tool::ToolIdentity;

async fn pinned_run() -> (Run, Arc<FakeSessions>) {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let plugin = stored_plugin("acme", &[]);
	config.agent_plugins.push(plugin_reference(&plugin));
	catalog.plugins.insert(plugin.plugin.digest.clone(), plugin);
	let saved = snapshot(&mut catalog, &config, false).await.unwrap();
	let sessions = Arc::new(FakeSessions::default());
	sessions.script("docs", Listed::Tools(vec![listed_tool("search")]));
	let mut run = run_of(saved);
	run.context.plugin_tools = Some(
		pin_for_run(
			sessions.as_ref(),
			run.id,
			run.context.binding_snapshot.as_deref().unwrap(),
		)
		.await
		.unwrap(),
	);
	(run, sessions)
}

fn resolver(sessions: Option<Arc<FakeSessions>>) -> execution::PinnedResolver {
	let live = Arc::new(Live::new());
	execution::PinnedResolver {
		providers: live.clone(),
		authority: live,
		sessions: sessions.map(|sessions| sessions as Arc<dyn PluginSessions>),
	}
}

use crate::ports::tools::PluginSessions;

#[tokio::test]
async fn pinned_plugin_tools_are_offered_beside_the_bindings_with_their_specifications() {
	let (run, sessions) = pinned_run().await;

	let tools = resolver(Some(sessions)).tools(&run).await.unwrap();

	assert!(tools.contains_key("workspace_read"));
	let search = &tools["acme__docs__search"];
	assert_eq!(
		json!(search.specification()),
		json!({
			"name": "acme__docs__search",
			"description": "Does search.",
			"parameters": listed_tool("search").input_schema,
		})
	);
	let contract = search.contract();
	assert!(matches!(contract.identity, ToolIdentity::AgentPlugin(_)));
	assert!(contract.behavior.tool_call_approval);
	assert!(!search.replay_safe());
}

#[tokio::test]
async fn without_sessions_no_plugin_tool_is_offered() {
	let (run, _) = pinned_run().await;

	let tools = resolver(None).tools(&run).await.unwrap();

	assert!(tools.contains_key("workspace_read"));
	assert!(!tools.keys().any(|alias| alias.starts_with("acme__")));
}

#[tokio::test]
async fn a_run_that_has_not_pinned_yet_offers_no_plugin_tool() {
	let (mut run, sessions) = pinned_run().await;
	run.context.plugin_tools = None;

	let tools = resolver(Some(sessions)).tools(&run).await.unwrap();

	assert!(!tools.keys().any(|alias| alias.starts_with("acme__")));
}

#[tokio::test]
async fn a_plugin_tool_never_replaces_the_tool_of_a_binding() {
	let (mut run, sessions) = pinned_run().await;
	run.context.plugin_tools.as_mut().unwrap().servers[0].tools[0].alias = "workspace_read".into();

	let result = resolver(Some(sessions)).tools(&run).await;

	assert!(
		matches!(result, Err(Error::Conflict(_))),
		"{:?}",
		result.err()
	);
}
