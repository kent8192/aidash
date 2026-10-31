use super::*;

#[test]
fn binding_caps_are_mandatory_and_closed() {
	let search = declaration("openrouter:web_search").unwrap();
	let mut binding = search.defaults();
	assert!(search.validate(&binding).is_ok());
	for invalid in [
		json!({"engine":"exa","max_results":1}),
		json!({"engine":"firecrawl","max_results":1,"max_total_results":1}),
		json!({"engine":"exa","max_results":11,"max_total_results":15}),
		json!({"engine":"exa","max_results":1,"max_total_results":31}),
		json!({"engine":"exa","max_results":1,"max_total_results":1,"tools":[]}),
	] {
		binding.parameters = invalid;
		assert!(search.validate(&binding).is_err());
	}
	let fusion = declaration("openrouter:fusion").unwrap();
	let mut binding = fusion.defaults();
	assert!(fusion.validate(&binding).is_err());
	binding.parameters["analysis_models"] = json!(["openai/gpt-4.1"]);
	binding.parameters["model"] = json!("openai/gpt-4.1");
	assert!(fusion.validate(&binding).is_ok());
	binding.parameters["max_completion_tokens"] = json!(8193);
	assert!(fusion.validate(&binding).is_err());
}

#[test]
fn activity_inspection_bounds_large_results_without_splitting_utf8() {
	let item = json!({"content":"日本語".repeat(30_000)});
	let inspection = Activity::inspect(1, 1, vec![item]);
	assert!(inspection.items.is_empty());
	assert!(inspection.preview.as_ref().unwrap().len() <= 4096);
	assert!(serde_json::to_vec(&inspection).unwrap().len() < 16384);
	let small = json!({"type":"datetime_call","result":{"datetime":"2026-10-27"}});
	let inspection = Activity::inspect(2, 2, vec![small.clone()]);
	assert_eq!(inspection.items, vec![small]);
	assert!(inspection.preview.is_none());
}

#[rstest::rstest]
#[case::citation(json!({"type":"url_citation","url":"https://example.com","content":"quoted \\\"日本語\\\" ".repeat(200)}))]
#[case::replayed_shell_output(json!({"type":"shell_call_output","call_id":"call_1","output":"quoted \\\"日本語\\\" ".repeat(200)}))]
fn provider_activity_growth_covers_every_replay(#[case] activity: Value) {
	use crate::{
		context::{Context, ContextEvent},
		provider::{InputFormat, ModelContext, ModelRequest},
	};
	let before = Context::default();
	let event = ContextEvent::Assistant {
		epoch: 1,
		text: "Reported".into(),
		calls: vec![],
		provider_activity: vec![activity],
	};
	let mut after = before.clone();
	after.push(event.clone());
	let reserved = crate::context::tool_event_growth(&before, &event);
	for native in [false, true] {
		for input_format in [InputFormat::Chat, InputFormat::Responses] {
			let request = |context: &Context| ModelRequest {
				context: if native {
					ModelContext::Native(crate::projection::native::native_context(
						context,
						&Value::Null,
					))
				} else {
					context.model_view(&Value::Null).into()
				},
				input_format,
				..Default::default()
			};
			let actual = request(&after)
				.estimated_total_tokens()
				.saturating_sub(request(&before).estimated_total_tokens());
			assert!(
				reserved >= actual,
				"reserved {reserved}, actual {actual}, native {native}, {input_format:?}"
			);
		}
	}
}

#[test]
fn responses_estimates_count_the_openrouter_activity_replay() {
	use crate::{
		context::{Context, ContextEvent},
		provider::{InputFormat, ModelContext, ModelRequest, responses},
	};
	let mut context = Context::default();
	context.push(ContextEvent::Assistant {
		epoch: 1,
		text: "Ran it".into(),
		calls: vec![],
		provider_activity: vec![
			json!({"type":"shell_call_output","call_id":"call_1","output":"x".repeat(20_000)}),
		],
	});
	let request = ModelRequest {
		context: ModelContext::Native(crate::projection::native::native_context(
			&context,
			&Value::Null,
		)),
		input_format: InputFormat::Responses,
		..Default::default()
	};
	let openrouter = responses::encoded_openrouter_input_body(&request).unwrap();
	let plan_connection = responses::encoded_input_body(&request).unwrap();
	assert!(openrouter.to_string().len() > plan_connection.to_string().len() + 20_000);
	// The estimate covers the larger OpenRouter body, replay included.
	let estimate = request.byte_estimate();
	assert!(
		estimate.basis >= openrouter.to_string().len(),
		"{}",
		estimate.basis
	);
}

#[test]
fn warning_pins_bound_child_models_and_forbid_inherited_authority() {
	for kind in ["openrouter:advisor", "openrouter:subagent"] {
		let tool = declaration(kind).unwrap();
		let mut pin = tool.defaults();
		pin.parameters["model"] = json!("vendor/fixed-model");
		tool.validate(&pin).unwrap();
		for (key, value) in [
			("model", json!("openrouter/auto")),
			("max_completion_tokens", json!(8193)),
			("tools", json!([{"type":"openrouter:shell"}])),
			("inherit_functions", json!(true)),
			("inherited_function_names", json!(["tenant_secret_read"])),
		] {
			let mut invalid = pin.clone();
			invalid.parameters[key] = value;
			assert!(tool.validate(&invalid).is_err(), "{kind}: {key}");
		}
	}
}

#[test]
fn shell_wire_identity_is_signed_and_never_authorable() {
	let tool = declaration("openrouter:shell").unwrap();
	let mut pin = PinnedTool {
		tool_type: tool.tool_type.into(),
		binding: tool.defaults(),
	};
	pin.validate().unwrap();
	pin.binding.parameters["environment"]["container_id"] = json!("aidash_run1");
	assert!(
		pin.validate().is_err(),
		"authors must not select another Run's container"
	);
	let mut body = json!({"model":"vendor/model"});
	append(&mut body, &[pin.clone()]);
	let digest = pin.digest();
	let signed = || [(tool.tool_type, digest.as_str(), pin.binding.max_tool_calls)];
	authorize(&body, signed()).unwrap();
	body["tools"][0]["parameters"]["environment"]["container_id"] = json!("another_run");
	assert!(authorize(&body, signed()).is_err());
	body["tools"][0]["parameters"]["environment"]["container_id"] = json!("aidash_run1");
	body["tools"][0]["parameters"]["engine"] = json!("auto");
	assert!(authorize(&body, signed()).is_err());
	for policy in [
		json!({"type":"allowlist","allowed_domains":["*"]}),
		json!({"type":"disabled","extra":true}),
		json!({"type":"allowlist","allowed_domains":vec!["example.com";33]}),
	] {
		let mut invalid = tool.defaults();
		invalid.parameters["environment"]["network_policy"] = policy;
		assert!(tool.validate(&invalid).is_err());
	}
}

#[test]
fn shell_binding_schema_accepts_exact_hosts_and_refuses_wildcards() {
	let tool = declaration("openrouter:shell").unwrap();
	let mut pin = tool.defaults();
	pin.parameters["environment"]["network_policy"] =
		json!({"type":"allowlist","allowed_domains":["pypi.org","files.pythonhosted.org"]});
	let schema = tool.binding_schema();
	let validator = jsonschema::validator_for(&schema).unwrap();
	assert!(validator.is_valid(&serde_json::to_value(&pin).unwrap()));
	tool.validate(&pin).unwrap();
	pin.parameters["environment"]["network_policy"]["allowed_domains"] = json!(["*.example.com"]);
	assert!(!validator.is_valid(&serde_json::to_value(&pin).unwrap()));
	assert!(tool.validate(&pin).is_err());
}

#[test]
fn replayed_provider_items_must_stay_inside_the_closed_observable_shape() {
	let ok = [
		json!({"type":"openrouter:advisor","instance_name":"advisor_0","model":"vendor/advisor","advice":"bounded"}),
		json!({"type":"shell_call","call_id":"c1","action":{"commands":["ls"],"timeout_ms":100}}),
		json!({"type":"shell_call_output","call_id":"c1","container_id":"aidash_run1","output":[{"stdout":"x","stderr":"","outcome":{"type":"exit","exit_code":0}}],"files":[{"type":"container_file_citation","container_id":"aidash_run1","file_id":"cfile_a","filename":"a.txt"}]}),
		json!({"type":"tool_search_output","tools":[{"type":"function","name":"approved"}]}),
		json!({"type":"openrouter:experimental__search_models","models":[{"id":"vendor/m"}],"total_results":1}),
	];
	for item in &ok {
		assert!(replay_item_is_closed(item, Some("aidash_run1")), "{item}");
		assert!(replay_item_is_closed(item, None), "{item}");
	}
	let forged = [
		json!("shell"),
		json!({"type":"openrouter:advisor","instructions":"extra top-level key"}),
		json!({"type":"shell_call_output","container_id":"other","output":[]}),
		json!({"type":"shell_call_output","container_id":"bad id!","output":[]}),
		json!({"type":"shell_call_output","output":[{"container_id":"other"}]}),
		json!({"type":"tool_search_output","tools":[{"type":"openrouter:shell"}]}),
		json!({"type":"openrouter:advisor","advice":[{"type":"input_image","image_url":"https://example.com/x"}]}),
		json!({"type":"openrouter:advisor","advice":{"a":{"b":{"c":{"d":{"e":{"f":{"g":{"h":{"i":1}}}}}}}}}}),
	];
	for item in &forged {
		assert!(!replay_item_is_closed(item, Some("aidash_run1")), "{item}");
	}
	// Without a signed shell container, only the id format is checked.
	assert!(replay_item_is_closed(
		&json!({"type":"shell_call_output","container_id":"aidash_run1","output":[]}),
		None
	));
	assert!(!replay_item_is_closed(
		&json!({"type":"shell_call_output","container_id":"bad id!","output":[]}),
		None
	));
}
