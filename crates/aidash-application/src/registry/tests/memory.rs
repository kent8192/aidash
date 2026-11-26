//! The Registry installs built-in memory implicitly and refuses explicit pins.
use super::*;

#[rstest]
#[case::custom("custom-memory")]
#[case::built_in("aidash.memory")]
#[tokio::test]
async fn registry_rejects_explicit_agent_memory_bindings(
	validation: DefinitionValidation,
	#[case] target: &str,
) {
	let mut scope = Scope::default();
	scope.put_builtins("aidash://home");
	scope.put(definition("model", "model", json!({"provider":"openai","model_id":"fixture","endpoint":"https://model.example.test","context_window":32768,"modalities":["text"],"cost":{}})));
	let agent = definition(
		"agent",
		"agent",
		json!({
			"model":{"id":"model","version":"1.0.0"},
			"instructions":"Use current memory",
			"bindings":[crate::test_support::binding("memory", "aidash://home", target)],
		}),
	);
	let error = register_definition(&mut scope, &validation, &agent, "aidash://home")
		.await
		.unwrap_err();
	assert!(
		error
			.to_string()
			.contains("memory and approval judging are built in")
	);
	assert!(!scope.entries.contains_key("agent@1.0.0"));
}
