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

#[test]
fn provider_activity_growth_covers_both_legacy_and_native_replay() {
	use crate::{
		context::{Context, ContextEvent},
		provider::{ModelContext, ModelRequest},
	};
	let before = Context::default();
	let event = ContextEvent::Assistant {
		epoch: 1,
		text: "Reported".into(),
		calls: vec![],
		provider_activity: vec![
			json!({"type":"url_citation","url":"https://example.com","content":"quoted \\\"日本語\\\" ".repeat(200)}),
		],
	};
	let mut after = before.clone();
	after.push(event.clone());
	let reserved = crate::context::tool_event_growth(&before, &event);
	for native in [false, true] {
		let request = |context: &Context| ModelRequest {
			context: if native {
				ModelContext::Native(crate::projection::native::native_context(
					context,
					&Value::Null,
				))
			} else {
				context.model_view(&Value::Null).into()
			},
			..Default::default()
		};
		let actual = request(&after)
			.estimated_total_tokens()
			.saturating_sub(request(&before).estimated_total_tokens());
		assert!(
			reserved >= actual,
			"reserved {reserved}, actual {actual}, native {native}"
		);
	}
}
