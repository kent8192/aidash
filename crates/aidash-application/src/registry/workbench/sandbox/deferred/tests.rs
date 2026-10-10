use super::*;

fn snapshot() -> BindingSnapshot {
	let mut agent = crate::test_support::agent("agent");
	agent.config["exposure"] = json!({"version":"deferred@1"});
	crate::test_support::resolve("aidash://local", &agent, false, vec![])
}
fn binding<'a>(snapshot: &'a BindingSnapshot, alias: &str) -> &'a ResolvedBinding {
	snapshot
		.bindings
		.iter()
		.find(|binding| binding.alias.as_deref() == Some(alias))
		.unwrap()
}
fn call(name: &str, arguments: Value) -> ToolCall {
	ToolCall {
		id: "call".into(),
		name: name.into(),
		arguments,
	}
}

#[test]
fn exposure_tools_are_evaluated_and_staged_while_other_calls_follow_their_request() {
	// Arrange
	let snapshot = snapshot();
	let deferred = Deferred::new(&snapshot).unwrap().unwrap();
	let digest = binding(&snapshot, "workspace_observe").digest.clone();
	let mut state = ExposureState::default();
	let evaluate = |name: &str, arguments: Value, state: &mut ExposureState| {
		deferred
			.evaluate(
				binding(&snapshot, name),
				&call(name, arguments),
				state,
				0,
				&|_, _| true,
			)
			.unwrap()
			.map(|evaluated| match evaluated {
				Evaluated::Result(output) => output,
				Evaluated::NoRoom => panic!("an unlimited session always has room"),
			})
	};
	// Act / Assert: an unadvertised capability is answered, not simulated.
	assert_eq!(
		evaluate("workspace_observe", json!({}), &mut state),
		Some(json!({"error":"capability workspace_observe is not loaded; use capability_load"}))
	);
	assert_eq!(
		evaluate(
			"capability_load",
			json!({"alias":"workspace_observe","digest":"stale"}),
			&mut state
		),
		Some(json!({"error":"CAPABILITY_CHANGED"}))
	);
	assert!(state.is_empty());
	let loaded = evaluate(
		"capability_load",
		json!({"alias":"workspace_observe","digest":digest}),
		&mut state,
	)
	.unwrap();
	assert_eq!(loaded["status"], "loaded");
	assert_eq!(state.pending.len(), 1);
	assert_eq!(
		evaluate(
			"capability_search",
			json!({"query":"workspace_observe"}),
			&mut state
		)
		.unwrap()["results"][0]["loaded"],
		true
	);
	state.activate();
	assert_eq!(evaluate("workspace_observe", json!({}), &mut state), None);
	assert_eq!(
		evaluate(
			"capability_unload",
			json!({"alias":"workspace_observe"}),
			&mut state
		)
		.unwrap()["status"],
		"unloaded"
	);
	// The unload applies to the next request only.
	assert_eq!(evaluate("workspace_observe", json!({}), &mut state), None);
	state.activate();
	let (text, tools) = deferred.request(&snapshot, &state).unwrap();
	assert!(!tools.iter().any(|tool| tool.name == "workspace_observe"));
	assert!(text.contains("workspace_observe [tool]"));
}

#[test]
fn descriptions_and_search_pages_are_fitted_before_the_session_retains_them() {
	// Arrange: the next test request carries at most `limit` result bytes.
	let snapshot = snapshot();
	let deferred = Deferred::new(&snapshot).unwrap().unwrap();
	let evaluate = |name: &str, arguments: Value, limit: usize| {
		let fits = |result: &Value, reserve: usize| result.to_string().len() + reserve <= limit;
		deferred
			.evaluate(
				binding(&snapshot, name),
				&call(name, arguments),
				&mut ExposureState::default(),
				0,
				&fits,
			)
			.unwrap()
			.unwrap()
	};
	let result = |evaluated| match evaluated {
		Evaluated::Result(output) => output,
		Evaluated::NoRoom => panic!("expected a retained result"),
	};
	let describe = |limit| {
		evaluate(
			"capability_describe",
			json!({"alias":"workspace_observe"}),
			limit,
		)
	};
	let whole = result(describe(usize::MAX));
	let bytes = whole["bytes"].as_u64().unwrap() as usize;
	let needed = whole.to_string().len() + bytes;
	// Act / Assert: a description is kept only with room for its later Load.
	assert_eq!(result(describe(needed)), whole);
	let omitted = result(describe(needed - 1));
	assert!(omitted.get("detail").is_none(), "{omitted}");
	assert_eq!(omitted["deferred"], true);
	assert_eq!(
		(&omitted["alias"], &omitted["digest"], &omitted["bytes"]),
		(&whole["alias"], &whole["digest"], &whole["bytes"])
	);
	// Not even the omitted description fits: nothing is retained.
	assert_eq!(describe(omitted.to_string().len() - 1), Evaluated::NoRoom);
	// A search page keeps only its leading results that fit.
	let page = result(evaluate("capability_search", json!({}), usize::MAX));
	let results = page["results"].as_array().unwrap();
	assert!(results.len() > 1);
	let first = crate::execution::capability_search_result(&page, 0, 1);
	let cut = result(evaluate(
		"capability_search",
		json!({}),
		first.to_string().len(),
	));
	assert_eq!(cut, first);
	assert_eq!(cut["next_cursor"], "1");
	let empty = crate::execution::capability_search_result(&page, 0, 0);
	let deferred_page = result(evaluate(
		"capability_search",
		json!({}),
		empty.to_string().len(),
	));
	assert_eq!(deferred_page, empty);
	assert_eq!(deferred_page["deferred"], true);
	// Not even an empty page fits: nothing is retained.
	assert_eq!(
		evaluate("capability_search", json!({}), empty.to_string().len() - 1),
		Evaluated::NoRoom
	);
}

#[test]
fn replay_uses_only_results_the_session_evaluated() {
	// Arrange
	let update = |alias: &str| json!({"load":{"alias":alias,"kind":"tool","identity":{"registry":{"registry_node":"aidash://local","id":alias,"version":"1.0.0"}},"digest":"d","step":0}});
	let conversation = [
		json!({"role":"user","content":"start"}),
		json!({"role":"tool","content":{"outcome":EVALUATED,"result":{"exposure_update":update("evaluated")}}}),
		json!({"role":"tool","content":{"outcome":"simulated","fixture":{"response":{"exposure_update":update("fixture")}}}}),
		json!({"role":"tool","content":{"outcome":EVALUATED,"result":{"exposure_update":{"unload":{"alias":"evaluated"}}}}}),
		json!({"role":"tool","content":{"outcome":EVALUATED,"result":{"exposure_update":update("again")}}}),
	];
	// Act
	let state = replayed(&conversation).unwrap();
	// Assert
	assert_eq!(
		state
			.loaded
			.iter()
			.map(|loaded| loaded.alias.as_str())
			.collect::<Vec<_>>(),
		["again"]
	);
	assert!(state.unloaded_eager.contains("evaluated"));
	assert!(state.pending.is_empty());
}
