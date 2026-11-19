use super::*;

fn list(patterns: &[&str]) -> ToolList {
	ToolList::new(patterns.iter().map(|p| ToolPattern::new(*p).unwrap())).unwrap()
}

#[test]
fn patterns_match_names_and_globs() {
	let file = ToolPattern::new("file_*").unwrap();
	assert!(file.matches("file_read"));
	assert!(!file.matches("shell"));
	let github = ToolPattern::new("registry:github/*").unwrap();
	assert!(github.matches("registry:github/create_issue"));
	assert!(!github.matches("registry:gitlab/create_issue"));
	assert!(ToolPattern::new("*").unwrap().matches("anything/at:all"));
	assert!(ToolPattern::new("a*b*c").unwrap().matches("axxbyyc"));
	assert!(!ToolPattern::new("a*b*c").unwrap().matches("axxbyy"));
	assert!(ToolPattern::new("shell").unwrap().matches("shell"));
	assert!(!ToolPattern::new("shell").unwrap().matches("shell_poll"));
	// MCP tool names keep their case, and matching respects it.
	let weather = ToolPattern::new("registry:weather/getWeather").unwrap();
	assert!(weather.matches("registry:weather/getWeather"));
	assert!(!weather.matches("registry:weather/getweather"));
}

#[test]
fn patterns_reject_bad_input() {
	for bad in ["", "a b", "a,b", "x;y", &"a".repeat(MAX_PATTERN_BYTES + 1)] {
		assert!(ToolPattern::new(bad).is_err(), "{bad:?}");
	}
	assert!(
		HostPattern::new("*.github.com")
			.unwrap()
			.matches("API.github.com")
	);
	assert!(HostPattern::new("bad/host").is_err());
}

#[test]
fn lists_sort_dedupe_and_cap() {
	let l = list(&["b", "a", "b"]);
	assert_eq!(l.patterns().len(), 2);
	assert_eq!(l.patterns()[0].as_str(), "a");
	let too_many = (0..=MAX_PATTERNS).map(|i| ToolPattern::new(format!("t{i}")).unwrap());
	assert!(ToolList::new(too_many).is_err());
	assert!(ToolList::defaults().matches("skill_read"));
	assert!(!ToolList::defaults().matches("file_read"));
}

#[test]
fn both_keys_decide_and_the_block_wins() {
	let request = list(&["file_*", "shell"]);
	let allowance = list(&["file_read", "file_search", "shell"]);
	let block = list(&["shell"]);
	let empty = ToolList::default();
	let lists = RemoteToolLists {
		request: &request,
		allowance: &allowance,
		block: &block,
		run_request: &empty,
		run_allowance: &empty,
	};
	assert_eq!(
		lists.decide(ToolReach::ReceiverMaterial, "file_read"),
		ToolVerdict::Usable
	);
	assert_eq!(
		lists.decide(ToolReach::Execution, "shell"),
		ToolVerdict::Unavailable
	);
	assert_eq!(
		lists.decide(ToolReach::HomeWorkspace, "file_share"),
		ToolVerdict::Ask(Lacking {
			home: false,
			receiver: true
		})
	);
	assert_eq!(
		lists.decide(ToolReach::Execution, "code_interpreter"),
		ToolVerdict::Ask(Lacking {
			home: true,
			receiver: true
		})
	);
	assert_eq!(
		lists.decide(ToolReach::Run, "journal_read"),
		ToolVerdict::Usable
	);
	assert_eq!(
		lists.decide(ToolReach::Never, "file_read"),
		ToolVerdict::Unavailable
	);
}

#[test]
fn run_only_answers_fill_the_missing_side() {
	let request = list(&["file_share"]);
	let empty = ToolList::default();
	let run_allowance = list(&["file_share"]);
	let lists = RemoteToolLists {
		request: &request,
		allowance: &empty,
		block: &empty,
		run_request: &empty,
		run_allowance: &run_allowance,
	};
	assert_eq!(
		lists.decide(ToolReach::HomeWorkspace, "file_share"),
		ToolVerdict::Usable
	);
}

#[test]
fn discovery_shows_the_allowance_minus_the_block() {
	let allowance = list(&["file_*"]);
	let block = list(&["file_share"]);
	assert!(RemoteToolLists::discoverable(
		&allowance,
		&block,
		"file_read"
	));
	assert!(!RemoteToolLists::discoverable(
		&allowance,
		&block,
		"file_share"
	));
	assert!(!RemoteToolLists::discoverable(&allowance, &block, "shell"));
}

#[test]
fn the_request_round_trips_as_an_authorization_detail() {
	let request = FederationToolRequest {
		tools: list(&["file_read"]),
		hosts: vec![HostPattern::new("*.example.com").unwrap()],
	};
	let detail = ToolAuthorizationDetail::new(&request);
	let json = serde_json::to_value(&detail).unwrap();
	assert_eq!(json["type"], AUTHORIZATION_DETAIL_TYPE);
	assert_eq!(json["actions"][0], "file_read");
	assert_eq!(json["locations"][0], "*.example.com");
	let back: ToolAuthorizationDetail = serde_json::from_value(json).unwrap();
	assert_eq!(back.request().unwrap(), request);
	assert!(request.allows_host("api.example.com"));
	assert!(!request.allows_host("example.org"));
	let other = ToolAuthorizationDetail {
		kind: "other".into(),
		..detail
	};
	assert!(other.request().is_err());
}

#[test]
fn list_key_names_builtins_plainly_and_registry_tools_by_definition_and_operation() {
	use crate::registry::{EntityRef, bindings::QualifiedRef};
	use crate::tool::builtin_contract;
	let shell = builtin_contract("shell").unwrap();
	assert_eq!(list_key(&shell, "ignored"), "shell");
	let reference = EntityRef {
		id: "github".into(),
		version: "1.0.0".into(),
	};
	let config = ToolConfig::Mcp {
		endpoint: "https://example.test/mcp".into(),
		tool_name: "create_issue".into(),
		replay: "idempotent".into(),
		idempotency_argument: None,
		secret: None,
		scopes: None,
		secret_owner: Default::default(),
	};
	let registry = ToolContract::registry(reference, &config);
	let operation = registry_operation(&config);
	assert_eq!(
		list_key(&registry, operation),
		"registry:github/create_issue"
	);
	// A descriptor of a core provider is its builtin; any other is a Registry key.
	let qualified = QualifiedRef {
		registry_deployment: "deployment".into(),
		id: "aidash.shell".into(),
		version: "1.0.0".into(),
	};
	let mut core = shell.clone();
	core.identity = ToolIdentity::Descriptor(qualified.clone());
	assert_eq!(list_key(&core, "shell"), "shell");
	assert_eq!(list_key(&core, "lookup"), "registry:aidash.shell/lookup");
}

#[test]
fn a_message_with_files_needs_its_own_key_after_the_plain_one() {
	use serde_json::json;
	let keys = |key: &str, arguments| call_keys(key.to_owned(), &arguments);
	for plain in [
		json!({"content":"hi"}),
		json!({"content":"hi","files":null}),
		json!({"content":"hi","files":[]}),
	] {
		assert_eq!(keys("workspace_message", plain), ["workspace_message"]);
	}
	// Anything else in `files`, malformed included, is a message with files.
	for files in [json!(["out/a.pdf"]), json!("out/a.pdf")] {
		assert_eq!(
			keys("workspace_message", json!({"content":"hi","files":files})),
			["workspace_message", WORKSPACE_MESSAGE_FILES]
		);
	}
	assert_eq!(keys("file_share", json!({"files":["a"]})), ["file_share"]);
	// The defaults hold the plain message only; a pattern of the tool names both forms.
	assert!(ToolList::defaults().matches("workspace_message"));
	assert!(!ToolList::defaults().matches(WORKSPACE_MESSAGE_FILES));
	assert!(list(&["workspace_message*"]).matches(WORKSPACE_MESSAGE_FILES));
	assert!(ToolPattern::new(WORKSPACE_MESSAGE_FILES).is_ok());
}

#[test]
fn a_run_only_answer_lets_the_call_through_without_changing_the_registered_lists() {
	let allowance = list(&["workspace_read"]);
	let run_only = list(&["registry:github/create_issue"]);
	let lists = RemoteToolLists {
		request: &list(&["workspace_read", "registry:github/*"]),
		allowance: &allowance,
		block: &ToolList::default(),
		run_request: &ToolList::default(),
		run_allowance: &run_only,
	};
	assert_eq!(
		lists.decide(ToolReach::ExternalServices, "registry:github/create_issue"),
		ToolVerdict::Usable
	);
	// Another tool of the same definition is still asked about: the answer named one tool.
	assert_eq!(
		lists.decide(ToolReach::ExternalServices, "registry:github/delete_repo"),
		ToolVerdict::Ask(Lacking {
			home: false,
			receiver: true
		})
	);
	assert_eq!(allowance, list(&["workspace_read"]));
}

#[test]
fn a_blocked_tool_is_not_let_through_by_a_run_only_answer() {
	let run_only = list(&["shell"]);
	let lists = RemoteToolLists {
		request: &run_only,
		allowance: &ToolList::default(),
		block: &list(&["shell"]),
		run_request: &run_only,
		run_allowance: &run_only,
	};
	assert_eq!(
		lists.decide(ToolReach::Execution, "shell"),
		ToolVerdict::Unavailable
	);
}

#[test]
fn the_homes_run_only_tools_travel_inside_the_request() {
	let request = FederationToolRequest {
		tools: list(&["workspace_read"]),
		hosts: vec![HostPattern::new("example.com").unwrap()],
	};
	let merged = request
		.with_run_tools(&list(&["shell", "workspace_read"]))
		.unwrap();
	assert_eq!(merged.tools, list(&["shell", "workspace_read"]));
	assert_eq!(merged.hosts, request.hosts);
	assert_eq!(request.tools, list(&["workspace_read"]));
	assert_eq!(
		request.with_run_tools(&ToolList::default()).unwrap(),
		request
	);
}

#[test]
fn an_answer_lists_the_exact_key_or_a_pattern_that_matches_it() {
	let key = "registry:github/create_issue";
	assert_eq!(answered_pattern(key, None).unwrap().as_str(), key);
	assert_eq!(answered_pattern(key, Some("  ")).unwrap().as_str(), key);
	assert_eq!(
		answered_pattern(key, Some("registry:github/*"))
			.unwrap()
			.as_str(),
		"registry:github/*"
	);
	// A pattern that misses the key would leave the Run paused on the same call.
	assert!(answered_pattern(key, Some("registry:gitlab/*")).is_err());
	assert!(answered_pattern(key, Some("Bad Pattern")).is_err());
	// The default answer for a paused MCP call lists its exact, case-kept key.
	let mcp = "registry:weather/getWeather";
	assert_eq!(answered_pattern(mcp, None).unwrap().as_str(), mcp);
}

#[test]
fn an_asked_key_that_names_no_single_tool_is_never_listed() {
	assert!(names_one_tool("shell"));
	assert!(names_one_tool("registry:weather/getWeather"));
	// A receiving Node that names `*` (or any pattern) to the Home cannot widen its Request.
	for key in ["*", "file_*", "registry:github/*", "", "a b"] {
		assert!(!names_one_tool(key), "{key:?}");
		assert!(answered_pattern(key, None).is_err(), "{key:?}");
		assert!(answered_pattern(key, Some("*")).is_err(), "{key:?}");
	}
}

fn paused_on(call: &str, key: &str, receiver: bool) -> crate::run_state::PausedCall {
	crate::run_state::PausedCall {
		key: call.into(),
		index: 0,
		replay_safe: true,
		tool: key.into(),
		kind: crate::tool::RefusalKind::OtherNode,
		tool_call: None,
		ask: Some(ToolAsk {
			key: key.into(),
			lacking: Lacking {
				home: !receiver,
				receiver,
			},
		}),
	}
}

#[test]
fn the_receiver_answers_only_its_own_ask_and_only_while_nothing_else_holds_the_run() {
	use crate::{ControlSource, RunControl};
	let paused = paused_on("call-1", "shell", true);
	let may = |control, source, paused: Option<&crate::run_state::PausedCall>, call, key| {
		receiver_may_answer(control, source, paused, call, key)
	};
	// The worker's Authority Pause leaves the source `System`.
	assert!(may(
		RunControl::Paused,
		ControlSource::System,
		Some(&paused),
		"call-1",
		"shell"
	));
	// The Home's people (or this Node's) paused it since, or a Safety Stop hit it.
	for source in [
		ControlSource::Tenant,
		ControlSource::DeploymentAdministrator,
	] {
		assert!(!may(
			RunControl::Paused,
			source,
			Some(&paused),
			"call-1",
			"shell"
		));
	}
	// Running again, or cancelled.
	for control in [RunControl::Active, RunControl::Cancelled] {
		assert!(!may(
			control,
			ControlSource::System,
			Some(&paused),
			"call-1",
			"shell"
		));
	}
	// A stale page: the Run now waits on another call, or another tool.
	assert!(!may(
		RunControl::Paused,
		ControlSource::System,
		Some(&paused),
		"call-2",
		"shell"
	));
	assert!(!may(
		RunControl::Paused,
		ControlSource::System,
		Some(&paused),
		"call-1",
		"file_read"
	));
	// No paused call, a pause that asks only the Home, or one that is no list ask at all.
	assert!(!may(
		RunControl::Paused,
		ControlSource::System,
		None,
		"call-1",
		"shell"
	));
	let home_only = paused_on("call-1", "shell", false);
	assert!(!may(
		RunControl::Paused,
		ControlSource::System,
		Some(&home_only),
		"call-1",
		"shell"
	));
	let no_ask = crate::run_state::PausedCall {
		ask: None,
		..paused.clone()
	};
	assert!(!may(
		RunControl::Paused,
		ControlSource::System,
		Some(&no_ask),
		"call-1",
		"shell"
	));
}

#[test]
fn an_agent_card_drops_the_core_capabilities_of_blocked_builtins() {
	assert!(blocked_core_capabilities(&ToolList::default()).is_empty());
	assert_eq!(blocked_core_capabilities(&list(&["shell"])), ["shell"]);
	// Any one tool of a flag clears it: `shell_poll` is turned on by `shell` too.
	assert_eq!(blocked_core_capabilities(&list(&["shell_poll"])), ["shell"]);
	assert_eq!(
		blocked_core_capabilities(&list(&["file_read", "outbound_get", "apply_patch"])),
		["files", "outbound", "patch"]
	);
	assert_eq!(
		blocked_core_capabilities(&list(&["code_*", "file_share", "skill_read"])),
		["python", "sharing", "skills"]
	);
	// Tools no flag turns on clear nothing; a Registry tool is not a builtin.
	assert!(blocked_core_capabilities(&list(&["task_create", "registry:github/*"])).is_empty());
}

#[test]
fn only_the_receiver_may_block() {
	assert!(AskChoice::Block.allowed_on(AskSide::Receiver));
	assert!(!AskChoice::Block.allowed_on(AskSide::Home));
	for choice in [AskChoice::RunOnly, AskChoice::Add, AskChoice::Decline] {
		assert!(choice.allowed_on(AskSide::Home));
		assert!(choice.allowed_on(AskSide::Receiver));
	}
	assert!(AskChoice::RunOnly.resumes_the_run());
	assert!(AskChoice::Add.resumes_the_run());
	assert!(AskChoice::Block.resumes_the_run());
	assert!(!AskChoice::Decline.resumes_the_run());
}

#[test]
fn an_answer_is_read_from_its_snake_case_name() {
	let choice: AskChoice = serde_json::from_str("\"run_only\"").unwrap();
	assert_eq!(choice, AskChoice::RunOnly);
	assert!(serde_json::from_str::<AskChoice>("\"always\"").is_err());
}

#[test]
fn a_list_drops_an_exact_entry_and_unions_with_another() {
	let a = list(&["a", "b"]);
	assert_eq!(a.without(&ToolPattern::new("a").unwrap()), list(&["b"]));
	assert_eq!(a.union(&list(&["b", "c"])).unwrap(), list(&["a", "b", "c"]));
	assert!(ToolList::default().is_empty());
}
