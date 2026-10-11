//! Registration fits every bound capability under its Exposure policy.
use super::*;
use crate::ports::{Credentials, registry::CoreToolCatalog};
use aidash_domain::{
	capabilities::CoreCapabilities, provider::ToolSpec, registry::bindings::BindingSnapshot,
};
use rstest::rstest;
use serde_json::json;
use std::collections::BTreeMap;

const NODE: &str = "aidash://local";

struct Contracts;
impl Credentials for Contracts {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("prompt budgeting cannot disclose credentials")
	}
}
impl CoreToolCatalog for Contracts {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		BTreeMap::new()
	}
}
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(Contracts), Arc::new(Contracts))
}

/// An Agent with one large tool and one large Registry Skill under `exposure`;
/// under `deferred@1` the tool is an Eager binding.
fn snapshot(
	exposure: Option<Value>,
	tool_pad: usize,
	skill_pad: usize,
	window: usize,
) -> BindingSnapshot {
	let mut root = crate::test_support::agent("agent");
	let mut tool_binding = crate::test_support::binding("tool", NODE, "large-tool");
	if let Some(exposure) = exposure {
		root.config["exposure"] = exposure;
		tool_binding["exposure"] = json!("eager");
	}
	let bindings = root.config["bindings"].as_array_mut().unwrap();
	bindings.push(tool_binding);
	bindings.push(crate::test_support::binding("skill", NODE, "large-skill"));
	let mut tool = crate::test_support::http_tool(NODE, "large-tool", "large_tool");
	tool.schema = json!({"type":"object","description":"x".repeat(tool_pad)});
	let skill = crate::test_support::entry(
		"large-skill",
		"skill",
		json!({"instructions":format!("Use it.{}", "y".repeat(skill_pad))}),
	);
	let model = crate::test_support::entry(
		"fixture-model",
		"model",
		json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://fixture.invalid","context_window":window,"max_output_tokens":1024,"modalities":["text"],"cost":{}}),
	);
	crate::test_support::resolve(NODE, &root, false, vec![tool, skill, model])
}
fn deferred(schema: usize, skill: usize) -> Option<Value> {
	Some(json!({"version":"deferred@1","schema_bytes":schema,"skill_bytes":skill}))
}

#[rstest]
fn deferred_registration_reserves_the_full_budgets_after_eager_exposure() {
	let legacy = validation()
		.bound_prompt_headroom(&snapshot(None, 10, 10, 200_000), &Value::Null)
		.unwrap();
	let headroom = validation()
		.bound_prompt_headroom(
			&snapshot(deferred(16_384, 32_768), 10, 10, 200_000),
			&Value::Null,
		)
		.unwrap();
	// The unused schema and Skill budgets and the whole metadata budget stay
	// reserved, so a deferred Agent keeps less history room than legacy.
	assert!(headroom + 4096 < legacy, "{headroom} vs {legacy}");
}

#[rstest]
fn an_oversized_single_tool_schema_is_rejected_by_alias_and_size() {
	let error = validation()
		.bound_prompt_headroom(
			&snapshot(deferred(16_384, 32_768), 20_000, 10, 400_000),
			&Value::Null,
		)
		.unwrap_err()
		.to_string();
	assert!(error.contains("tool large_tool needs"), "{error}");
	assert!(error.contains("schema_bytes budget of 16384"), "{error}");
}

#[rstest]
fn an_oversized_registry_skill_block_is_rejected_by_alias_and_size() {
	let error = validation()
		.bound_prompt_headroom(
			&snapshot(deferred(16_384, 2048), 10, 4000, 400_000),
			&Value::Null,
		)
		.unwrap_err()
		.to_string();
	assert!(error.contains("skill skill_large_skill_"), "{error}");
	assert!(error.contains("skill_bytes budget of 2048"), "{error}");
}

#[rstest]
fn mandatory_exposure_over_the_schema_budget_is_impossible() {
	let error = validation()
		.bound_prompt_headroom(
			&snapshot(deferred(1024, 32_768), 800, 10, 400_000),
			&Value::Null,
		)
		.unwrap_err()
		.to_string();
	assert!(error.contains("mandatory and eager exposure"), "{error}");
}

#[rstest]
fn deferred_budgets_must_fit_the_model_window_where_legacy_fits() {
	validation()
		.bound_prompt_headroom(&snapshot(None, 10, 10, 30_000), &Value::Null)
		.unwrap();
	let error = validation()
		.bound_prompt_headroom(
			&snapshot(deferred(16_384, 32_768), 10, 10, 30_000),
			&Value::Null,
		)
		.unwrap_err()
		.to_string();
	assert!(
		error.contains("deferred@1 exposure budgets cannot fit"),
		"{error}"
	);
}
