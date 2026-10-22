use super::*;
use crate::{
	ports::bindings::StoredAgentPlugin,
	test_support::{plugin_reference, stored_plugin},
};
use aidash_domain::tool::providers::core_descriptor;

const HOME: &str = "aidash://home";

fn scope_with_core() -> Scope {
	let mut scope = Scope::default();
	scope.put(definition(
		"model",
		"model",
		json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://fixture.invalid","context_window":200000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}),
	));
	for operation in system::operations() {
		scope.put(definition(
			&format!("aidash.{operation}"),
			"tool",
			serde_json::to_value(core_descriptor(HOME, operation).unwrap()).unwrap(),
		));
	}
	scope
}

fn agent_taking_in(plugin: &StoredAgentPlugin, exposure: Value) -> Entry {
	let mut config = json!({
		"schema_version": 1,
		"model": {"id": "model", "version": "1.0.0"},
		"instructions": "execute",
		"agent_plugins": [plugin_reference(plugin)],
	});
	if !exposure.is_null() {
		config["exposure"] = exposure;
	}
	definition("agent", "agent", config)
}

fn store(scope: &mut Scope, plugin: &StoredAgentPlugin) {
	scope
		.plugins
		.insert(plugin.plugin.digest.clone(), plugin.clone());
}

#[rstest]
#[tokio::test]
async fn an_agent_registers_with_the_plugin_copies_it_takes_in(validation: DefinitionValidation) {
	let mut scope = scope_with_core();
	let plugin = stored_plugin("acme-tools", &[("lint", "Lint it.")]);
	store(&mut scope, &plugin);

	let written = register_definition(
		&mut scope,
		&validation,
		&agent_taking_in(&plugin, Value::Null),
		HOME,
	)
	.await
	.unwrap();

	assert!(written);
	let stored = &scope.entries["agent@1.0.0"];
	assert_eq!(
		stored.config["agent_plugins"][0]["digest"],
		plugin.plugin.digest
	);
}

#[rstest]
#[tokio::test]
async fn registration_refuses_a_missing_copy_before_any_write(validation: DefinitionValidation) {
	let mut scope = scope_with_core();
	let plugin = stored_plugin("acme-tools", &[("lint", "Lint it.")]);

	let error = register_definition(
		&mut scope,
		&validation,
		&agent_taking_in(&plugin, Value::Null),
		HOME,
	)
	.await
	.unwrap_err();

	assert_eq!(error.to_string(), "AGENT_PLUGIN_MISSING: acme-tools");
	assert!(!scope.trace.contains(&"insert".to_owned()));
}

#[rstest]
#[tokio::test]
async fn registration_refuses_a_copy_that_differs_from_the_definition(
	validation: DefinitionValidation,
) {
	let mut scope = scope_with_core();
	let plugin = stored_plugin("acme-tools", &[("lint", "Lint it.")]);
	let mut changed = plugin.clone();
	changed.source = "https://github.com/mirror/tools/tree/abcdef".into();
	scope.plugins.insert(plugin.plugin.digest.clone(), changed);

	let error = register_definition(
		&mut scope,
		&validation,
		&agent_taking_in(&plugin, Value::Null),
		HOME,
	)
	.await
	.unwrap_err();

	assert!(matches!(error, Error::Conflict(_)), "{error:?}");
	assert!(!scope.trace.contains(&"insert".to_owned()));
}

#[rstest]
#[case::fits(100, true)]
#[case::over_the_skill_budget(4000, false)]
#[tokio::test]
async fn deferred_registration_budgets_plugin_skills_like_any_direct_skill(
	validation: DefinitionValidation,
	#[case] body: usize,
	#[case] accepted: bool,
) {
	let mut scope = scope_with_core();
	let plugin = stored_plugin("acme-tools", &[("lint", &"y".repeat(body))]);
	store(&mut scope, &plugin);
	let exposure = json!({"version": "deferred@1", "skill_bytes": 2048});

	let result = register_definition(
		&mut scope,
		&validation,
		&agent_taking_in(&plugin, exposure),
		HOME,
	)
	.await;

	assert_eq!(result.is_ok(), accepted, "{result:?}");
	if let Err(error) = result {
		assert!(
			error.to_string().contains("skill_bytes budget of 2048"),
			"{error}"
		);
	}
}
