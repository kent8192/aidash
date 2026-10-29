//! Agent Plugin tools (ADR 0073): a Run pins them once at its first step, and calls to them
//! wait for a Tool call approval unless the plugin declares them read-only.
use super::*;
use aidash_application::capabilities::approvals::Refusal;
use aidash_domain::{
	agent_plugin::{
		PinnedAgentPlugin, PinnedServer, PinnedTool, PluginName, PluginTools, ServerName,
		schema_digest,
	},
	tool::{AgentPluginTool, ToolContract},
};

const ALIAS: &str = "acme__docs__search";

fn digest() -> String {
	format!("sha256:{}", "0123456789abcdef".repeat(4))
}

fn plugin() -> PluginName {
	PluginName::try_from("acme".to_owned()).unwrap()
}

fn server() -> ServerName {
	ServerName::try_from("docs".to_owned()).unwrap()
}

fn schema() -> Value {
	json!({"type":"object","properties":{"q":{"type":"string"}}})
}

fn pinned(read_only: bool) -> PluginTools {
	PluginTools {
		servers: vec![PinnedServer {
			plugin: plugin(),
			digest: digest(),
			server: server(),
			tools: vec![PinnedTool {
				name: "search".into(),
				alias: ALIAS.into(),
				description: "Searches.".into(),
				input_schema: schema(),
				schema_digest: schema_digest(&schema()),
				read_only,
				eager: false,
			}],
		}],
		findings: vec![],
	}
}

fn identity() -> AgentPluginTool {
	AgentPluginTool {
		plugin: plugin(),
		digest: digest(),
		server: server(),
		tool: "search".into(),
	}
}

fn state(fixture: &mut Fixture) -> &mut State {
	Arc::get_mut(&mut fixture.backend.0).unwrap()
}

/// The Run's Agent takes in one plugin, and the environment lists `listing` for it.
fn with_plugin(fixture: &mut Fixture, listing: Option<PluginTools>) {
	let mut snapshot = DeferredSnapshot::default().snapshot();
	snapshot.agent_plugins = vec![PinnedAgentPlugin {
		digest: digest(),
		skills: vec![],
	}];
	fixture.run.context.binding_snapshot = Some(Box::new(snapshot));
	state(fixture).plugin_listing = listing;
}

fn listings(fixture: &Fixture) -> usize {
	fixture
		.backend
		.0
		.plugin_listings
		.load(std::sync::atomic::Ordering::SeqCst)
}

fn written(fixture: &Fixture, event: &str) -> Run {
	let writes = fixture.backend.0.writes.lock().unwrap();
	let (_, run) = writes
		.iter()
		.find(|(kind, _)| kind == event)
		.unwrap_or_else(|| panic!("{event} was not saved"));
	run.clone()
}

async fn advance(fixture: &mut Fixture) -> Result<()> {
	advance_sources(fixture).await
}

#[rstest]
#[tokio::test]
async fn the_first_step_pins_the_plugin_tools_once_before_the_run_starts(mut fixture: Fixture) {
	with_plugin(&mut fixture, Some(pinned(false)));

	advance(&mut fixture).await.unwrap();

	assert_eq!(listings(&fixture), 1);
	assert_eq!(
		fixture.backend.events(),
		["run.plugin_tools_pinned", "run.started"]
	);
	let pinning = written(&fixture, "run.plugin_tools_pinned");
	assert_eq!(pinning.context.plugin_tools, Some(pinned(false)));
	assert!(matches!(pinning.state, RunState::Ready(_)));
	let started = written(&fixture, "run.started");
	assert_eq!(started.context.plugin_tools, Some(pinned(false)));
	assert!(matches!(started.state, RunState::Thinking(_)));
	assert!(fixture.backend.0.closed_sessions.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn listing_holds_neither_the_authority_scope_nor_the_visibility_lock(mut fixture: Fixture) {
	with_plugin(&mut fixture, Some(pinned(false)));
	state(&mut fixture).scoped = true;

	advance(&mut fixture).await.unwrap();

	let calls = fixture.backend.calls();
	let listed = position(&calls, "plugins.list");
	assert_eq!(
		position(&calls, "authority.suspend") + 1,
		position(&calls, "visibility.suspend")
	);
	assert!(position(&calls, "visibility.suspend") < listed);
	assert!(listed < position(&calls, "visibility.resume"));
	assert!(position(&calls, "visibility.resume") < position(&calls, "authority.resume"));
	assert!(position(&calls, "authority.resume") < position(&calls, "save"));
}

#[rstest]
#[tokio::test]
async fn a_recovered_run_that_pinned_its_tools_never_lists_again(mut fixture: Fixture) {
	with_plugin(&mut fixture, Some(pinned(true)));
	fixture.run.context.plugin_tools = Some(pinned(false));

	advance(&mut fixture).await.unwrap();

	assert_eq!(listings(&fixture), 0);
	assert_eq!(fixture.backend.events(), ["run.started"]);
	assert_eq!(
		written(&fixture, "run.started").context.plugin_tools,
		Some(pinned(false))
	);
}

#[rstest]
#[tokio::test]
async fn a_crash_before_the_pin_is_saved_lists_again(mut fixture: Fixture) {
	with_plugin(&mut fixture, Some(pinned(false)));
	*state(&mut fixture).failing_save.lock().unwrap() = Some("run.plugin_tools_pinned");
	let stored = fixture.run.clone();

	assert!(advance(&mut fixture).await.is_err());
	fixture.run = stored;
	advance(&mut fixture).await.unwrap();

	assert_eq!(listings(&fixture), 2);
	assert_eq!(
		fixture.backend.events(),
		["run.plugin_tools_pinned", "run.started"]
	);
}

#[rstest]
#[tokio::test]
async fn a_run_without_plugins_lists_and_emits_nothing_new(mut fixture: Fixture) {
	state(&mut fixture).plugin_listing = Some(pinned(false));

	advance(&mut fixture).await.unwrap();

	assert_eq!(listings(&fixture), 0);
	assert_eq!(fixture.backend.events(), ["run.started"]);
	assert_eq!(written(&fixture, "run.started").context.plugin_tools, None);
}

#[rstest]
#[tokio::test]
async fn an_environment_without_plugin_support_pins_nothing(mut fixture: Fixture) {
	with_plugin(&mut fixture, None);

	advance(&mut fixture).await.unwrap();

	assert_eq!(listings(&fixture), 1);
	assert_eq!(fixture.backend.events(), ["run.started"]);
	assert_eq!(written(&fixture, "run.started").context.plugin_tools, None);
}

#[rstest]
#[case::completed(TaskStatus::Completed)]
#[case::failed(TaskStatus::Failed)]
#[case::cancelled(TaskStatus::Cancelled)]
#[tokio::test]
async fn a_run_that_reaches_a_terminal_phase_closes_its_plugin_sessions(
	mut fixture: Fixture,
	#[case] status: TaskStatus,
) {
	fixture.backend.0.task.lock().unwrap().status = status;

	advance(&mut fixture).await.unwrap();

	assert!(fixture.run.phase().is_terminal());
	assert_eq!(
		*fixture.backend.0.closed_sessions.lock().unwrap(),
		[fixture.run.id]
	);
}

fn plugin_call(fixture: &mut Fixture, read_only: bool) -> ToolCall {
	let specification = ToolSpec {
		name: ALIAS.into(),
		description: "Searches.".into(),
		parameters: schema(),
	};
	let tool = Arc::new(Scripted {
		specification,
		contract: ToolContract::agent_plugin(identity(), read_only),
		external_target: None,
		outputs: Mutex::new([json!({"content": []})].into()),
		invocations: Mutex::new(0),
		keys: Mutex::new(vec![]),
	});
	let state = state(fixture);
	state.custom = BTreeMap::from([(ALIAS.to_owned(), tool)]);
	state.scoped = true;
	state.permit_tools = true;
	let call = call("call-1", ALIAS, json!({"q": "rust"}));
	respond(&mut fixture.run, vec![call.clone()]);
	call
}

fn scripted(fixture: &Fixture) -> Arc<Scripted> {
	fixture.backend.0.custom[ALIAS].clone()
}

fn approvals(fixture: &Fixture, answers: impl IntoIterator<Item = ToolCallApproval>) {
	*fixture.backend.0.tool_call_approvals.lock().unwrap() = answers.into_iter().collect();
}

#[rstest]
#[tokio::test]
async fn a_preliminary_mcp_checkpoint_does_not_admit_stale_tool_calls(mut fixture: Fixture) {
	plugin_call(&mut fixture, true);
	approvals(&fixture, [ToolCallApproval::Approved { approval_id: None }]);
	fixture.run.state.tool_mut().unwrap().mcp =
		Some(aidash_domain::agent_plugin::interaction::CallCheckpoint::new("preliminary".into()));
	fixture
		.backend
		.add_input("Correct the request before calling the tool");

	advance(&mut fixture).await.unwrap();

	assert_eq!(scripted(&fixture).invocations(), 0);
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	assert_eq!(fixture.backend.events(), ["run.message_received"]);
	assert!(fixture.backend.0.invocations.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn a_rejected_call_returns_to_the_model_and_the_run_goes_on(mut fixture: Fixture) {
	plugin_call(&mut fixture, true);
	state(&mut fixture).reject_tools = Some("node_id \"x\" does not name this Node".into());

	advance(&mut fixture).await.unwrap();

	// The call never ran; the refusal is the call's result and the Run moves to the next call.
	assert_eq!(scripted(&fixture).invocations(), 0);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::Tool { result, .. })
			if *result == json!({"error":"node_id \"x\" does not name this Node"})
	));
	assert!(matches!(&saved.state, RunState::ToolCall(state) if state.cursor == 1));
	assert_ne!(fixture.run.control, RunControl::Paused);
}

#[rstest]
#[tokio::test]
async fn a_plugin_call_waits_for_a_tool_call_approval(mut fixture: Fixture) {
	let call = plugin_call(&mut fixture, false);
	let approval = Uuid::from_u128(0xa1);
	approvals(
		&fixture,
		[ToolCallApproval::Pending {
			approval_id: approval,
			expires_at: "2026-10-30T00:00:00Z".parse().unwrap(),
		}],
	);

	advance(&mut fixture).await.unwrap();

	assert_eq!(fixture.backend.events(), ["run.waiting_for_tool_approval"]);
	assert!(matches!(
		&fixture.run.state,
		RunState::Waiting(wait) if matches!(
			&**wait,
			WaitingState::Approval { approval: ApprovalRef::Capability { approval_id }, .. }
				if *approval_id == approval
		)
	));
	let requests = fixture.backend.0.tool_call_requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("one approval request");
	};
	assert_eq!(request.tool, ToolCallTarget::AgentPlugin(identity()));
	assert_eq!(request.tool.id(), "agent-plugin:acme/docs/search");
	assert_eq!(
		request.tool.label(),
		"agent-plugin:acme/docs/search@0123456789ab"
	);
	assert_eq!(request.operation, "invoke");
	assert_eq!(request.call, call);
	assert_eq!(scripted(&fixture).invocations(), 0);
}

#[rstest]
#[tokio::test]
async fn an_approved_plugin_call_runs_once(mut fixture: Fixture) {
	plugin_call(&mut fixture, false);
	approvals(&fixture, [ToolCallApproval::Approved { approval_id: None }]);

	advance(&mut fixture).await.unwrap();

	assert_eq!(scripted(&fixture).invocations(), 1);
	assert_eq!(
		scripted(&fixture).keys(),
		[format!("{}:{}:0", fixture.run.id, 0)]
	);
	assert!(
		!fixture
			.backend
			.events()
			.contains(&"run.waiting_for_tool_approval".into())
	);
}

#[rstest]
#[tokio::test]
async fn a_read_only_plugin_call_proceeds_without_approval(mut fixture: Fixture) {
	plugin_call(&mut fixture, true);
	// The gate only checks the Tenant's policy for a read-only tool.
	approvals(&fixture, [ToolCallApproval::Approved { approval_id: None }]);

	advance(&mut fixture).await.unwrap();

	assert_eq!(scripted(&fixture).invocations(), 1);
	let requests = fixture.backend.0.tool_call_requests.lock().unwrap();
	assert!(
		requests.iter().all(|request| request.read_only),
		"{requests:?}"
	);
	drop(requests);
	assert!(
		!fixture
			.backend
			.events()
			.contains(&"run.waiting_for_tool_approval".into())
	);
	assert!(fixture.backend.0.human.lock().unwrap().is_none());
}

#[rstest]
#[tokio::test]
async fn a_run_executing_for_another_node_asks_that_node_with_the_prompt_prefix_it_always_had(
	mut fixture: Fixture,
) {
	let call = plugin_call(&mut fixture, false);
	Arc::get_mut(&mut fixture.backend.0).unwrap().remote_home = true;

	advance(&mut fixture).await.unwrap();

	assert_eq!(fixture.backend.events(), ["run.waiting_for_tool_approval"]);
	assert!(
		fixture
			.backend
			.0
			.tool_call_requests
			.lock()
			.unwrap()
			.is_empty(),
		"a capability approval needs the executing Node's own authority"
	);
	let prompt = fixture
		.backend
		.0
		.human
		.lock()
		.unwrap()
		.as_ref()
		.expect("the Home Node is asked")
		.prompt
		.clone();
	assert_eq!(
		prompt,
		format!(
			"Approve this exact external tool action once? Tool: agent-plugin:acme/docs/search@0123456789ab; call: {}",
			serde_json::to_string(&call).unwrap()
		)
	);
}

#[rstest]
#[tokio::test]
async fn an_explicit_deny_refuses_a_read_only_plugin_call(mut fixture: Fixture) {
	plugin_call(&mut fixture, true);
	approvals(&fixture, [ToolCallApproval::Refused(Refusal::Forbidden)]);

	advance(&mut fixture).await.unwrap();

	assert_eq!(scripted(&fixture).invocations(), 0);
}
