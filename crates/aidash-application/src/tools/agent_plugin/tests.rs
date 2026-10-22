use super::*;
use crate::{
	plugin_tools::pin_for_run,
	test_support::{FakeSessions, Listed, listed_tool, plugin_snapshot, run_of, stored_plugin},
};
use aidash_domain::{agent_plugin::schema_digest, tool::ToolIdentity};
use serde_json::json;

struct Pinned {
	run: Run,
	sessions: Arc<FakeSessions>,
	tools: BTreeMap<String, Arc<dyn ExecutionTool>>,
}

async fn pinned(settings: Value) -> Pinned {
	let snapshot = plugin_snapshot(vec![stored_plugin("acme", &[])], settings);
	let sessions = Arc::new(FakeSessions::default());
	sessions.script(
		"docs",
		Listed::Tools(vec![listed_tool("search"), listed_tool("read")]),
	);
	let mut run = run_of(snapshot);
	let pin = pin_for_run(
		sessions.as_ref(),
		run.id,
		run.context.binding_snapshot.as_deref().unwrap(),
	)
	.await
	.unwrap();
	run.context.plugin_tools = Some(pin);
	let tools = offered(&run, &sessions);
	Pinned {
		run,
		sessions,
		tools,
	}
}

fn offered(run: &Run, sessions: &Arc<FakeSessions>) -> BTreeMap<String, Arc<dyn ExecutionTool>> {
	let sessions: Arc<dyn PluginSessions> = sessions.clone();
	pinned_tools(
		&sessions,
		run,
		run.context.binding_snapshot.as_deref().unwrap(),
	)
	.unwrap()
	.into_iter()
	.collect()
}

fn read_only_read() -> Value {
	json!({"acme": {"docs": {"read_only": [
		{"name": "read", "schema_digest": schema_digest(&listed_tool("read").input_schema)}
	]}}})
}

#[tokio::test]
async fn each_pinned_tool_is_offered_under_its_alias_with_the_pinned_schema() {
	let pinned = pinned(Value::Null).await;

	assert_eq!(
		pinned.tools.keys().collect::<Vec<_>>(),
		["acme__docs__read", "acme__docs__search"]
	);
	let search = &pinned.tools["acme__docs__search"];
	assert_eq!(
		json!(search.specification()),
		json!({
			"name": "acme__docs__search",
			"description": "Does search.",
			"parameters": listed_tool("search").input_schema,
		})
	);
	assert_eq!(
		json!(
			pinned_specifications(pinned.run.context.plugin_tools.as_ref())["acme__docs__search"]
		),
		json!(search.specification())
	);
}

#[tokio::test]
async fn a_plugin_tool_is_unsafe_and_asks_for_approval_unless_declared_read_only() {
	let pinned = pinned(read_only_read()).await;

	let search = pinned.tools["acme__docs__search"].contract();
	let read = pinned.tools["acme__docs__read"].contract();

	let ToolIdentity::AgentPlugin(identity) = &search.identity else {
		panic!("a plugin identity");
	};
	assert_eq!(identity.plugin.as_str(), "acme");
	assert_eq!(identity.server.as_str(), "docs");
	assert_eq!(identity.tool, "search");
	assert!(search.behavior.tool_call_approval);
	assert!(!read.behavior.tool_call_approval);
	for tool in pinned.tools.values() {
		assert!(!tool.replay_safe());
	}
}

#[tokio::test]
async fn a_call_runs_over_the_runs_session_with_the_pinned_server_and_schema_digest() {
	let pinned = pinned(Value::Null).await;
	pinned
		.sessions
		.reply(Ok(json!({"content": [{"text": "found"}]})));

	let result = pinned.tools["acme__docs__search"]
		.invoke(&pinned.run, json!({"q": "rust"}), "key-1")
		.await
		.unwrap();

	assert_eq!(result, json!({"content": [{"text": "found"}]}));
	let calls = pinned.sessions.calls();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0].run, pinned.run.id);
	assert_eq!(calls[0].server.plugin.as_str(), "acme");
	assert_eq!(calls[0].server.server.as_str(), "docs");
	assert_eq!(calls[0].url, "https://docs.example.com/mcp");
	assert_eq!(calls[0].tool, "search");
	assert_eq!(
		calls[0].schema_digest,
		schema_digest(&listed_tool("search").input_schema)
	);
	assert_eq!(calls[0].arguments, json!({"q": "rust"}));
	assert_eq!(calls[0].key, "key-1");
}

#[tokio::test]
async fn arguments_outside_the_pinned_schema_never_reach_the_server() {
	let pinned = pinned(Value::Null).await;

	for arguments in [json!({}), json!({"q": 7}), json!({"q": "x", "extra": true})] {
		let error = pinned.tools["acme__docs__search"]
			.invoke(&pinned.run, arguments, "key")
			.await
			.unwrap_err();
		assert!(matches!(error, Error::Invalid(_)), "{error}");
	}
	assert!(pinned.sessions.calls().is_empty());
}

#[tokio::test]
async fn a_changed_schema_comes_back_as_a_recoverable_invalid_error() {
	let pinned = pinned(Value::Null).await;
	pinned.sessions.reply(Err(Error::Invalid(
		"CAPABILITY_CHANGED: search changed its input schema".into(),
	)));

	let error = pinned.tools["acme__docs__search"]
		.invoke(&pinned.run, json!({"q": "rust"}), "key")
		.await
		.unwrap_err();

	assert!(
		matches!(&error, Error::Invalid(message) if message.starts_with("CAPABILITY_CHANGED")),
		"{error}"
	);
}

#[tokio::test]
async fn a_refused_destination_stays_an_authority_failure() {
	let pinned = pinned(Value::Null).await;
	pinned.sessions.reply(Err(Error::Forbidden));

	let error = pinned.tools["acme__docs__search"]
		.invoke(&pinned.run, json!({"q": "rust"}), "key")
		.await
		.unwrap_err();

	assert!(matches!(error, Error::Forbidden));
}

#[tokio::test]
async fn admission_refuses_a_tool_the_run_no_longer_pins() {
	let pinned = pinned(Value::Null).await;
	let tool = &pinned.tools["acme__docs__search"];
	let input = json!({"q": "rust"});

	let mut other_schema = pinned.run.clone();
	other_schema.context.plugin_tools.as_mut().unwrap().servers[0].tools[1].input_schema =
		json!({"type": "object"});
	let mut unpinned = pinned.run.clone();
	unpinned.context.plugin_tools = None;
	let mut foreign = pinned.run.clone();
	foreign.id = Uuid::new_v4();

	for run in [&other_schema, &unpinned, &foreign] {
		let error = tool.admit(run, input.clone()).await.unwrap_err();
		assert!(matches!(error, Error::Conflict(_)), "{error}");
	}
	assert!(tool.admit(&pinned.run, input).await.is_ok());
}

#[tokio::test]
async fn admission_refuses_a_plugin_copy_the_agent_no_longer_references() {
	let pinned = pinned(Value::Null).await;
	let mut run = pinned.run.clone();
	let snapshot = run.context.binding_snapshot.as_mut().unwrap();
	let agent = snapshot
		.definitions
		.iter_mut()
		.find(|definition| definition.identity == snapshot.agent)
		.unwrap();
	agent.definition.config["agent_plugins"] = json!([]);

	let error = pinned.tools["acme__docs__search"]
		.admit(&run, json!({"q": "rust"}))
		.await
		.unwrap_err();

	assert!(matches!(error, Error::Conflict(_)), "{error}");
}

#[tokio::test]
async fn a_run_that_pinned_nothing_offers_no_plugin_tool() {
	let pinned = pinned(Value::Null).await;
	let mut run = pinned.run.clone();
	run.context.plugin_tools = None;

	assert!(offered(&run, &pinned.sessions).is_empty());
	assert!(pinned_specifications(None).is_empty());
}

#[tokio::test]
async fn a_pin_naming_a_server_the_agent_does_not_declare_refuses_the_run() {
	let pinned = pinned(Value::Null).await;
	let mut run = pinned.run.clone();
	run.context.plugin_tools.as_mut().unwrap().servers[0].digest =
		format!("sha256:{}", "0".repeat(64));
	let sessions: Arc<dyn PluginSessions> = pinned.sessions.clone();

	let error = pinned_tools(
		&sessions,
		&run,
		run.context.binding_snapshot.as_deref().unwrap(),
	)
	.err()
	.expect("a refusal");

	assert!(matches!(error, Error::Conflict(_)), "{error}");
}
