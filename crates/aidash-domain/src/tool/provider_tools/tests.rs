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
