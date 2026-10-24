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
	DefinitionValidation::new(Arc::new(Contracts))
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

mod endpoints {
	use super::*;
	use crate::provider_access::KeylessAccess;
	use aidash_domain::agent_plugin::{McpServer, RemoteServer, ServerName};

	fn permitting(origins: &[&str]) -> DefinitionValidation {
		validation().with_keyless_origins(KeylessAccess::permitting(
			aidash_domain::keyless::PermittedOrigins::parse(origins).unwrap(),
		))
	}

	fn http(endpoint: &str) -> Value {
		json!({"transport": "http", "endpoint": endpoint, "replay": "read_only"})
	}

	fn mcp(endpoint: &str) -> Value {
		json!({"transport": "mcp", "endpoint": endpoint, "tool_name": "echo", "replay": "read_only"})
	}

	fn refusal(result: Result<()>) -> String {
		result.unwrap_err().to_string()
	}

	#[rstest]
	fn a_tool_endpoint_at_a_public_https_address_is_accepted(
		#[values(http, mcp)] transport: fn(&str) -> Value,
	) {
		for endpoint in [
			"https://tools.example.com/rpc",
			"https://tools.example.com:8443/rpc",
			"https://8.8.8.8/rpc",
		] {
			validation()
				.validate_tool(&transport(endpoint))
				.unwrap_or_else(|error| panic!("{endpoint}: {error}"));
		}
	}

	#[rstest]
	fn a_tool_endpoint_at_a_non_public_address_literal_is_refused(
		#[values(http, mcp)] transport: fn(&str) -> Value,
	) {
		for endpoint in [
			"http://127.0.0.1:8080/rpc",
			"https://10.0.0.5/rpc",
			"https://192.168.1.2/rpc",
			"http://169.254.169.254/latest",
			"https://[::1]/rpc",
		] {
			let message = refusal(validation().validate_tool(&transport(endpoint)));
			assert!(
				message.contains("TOOL_ENDPOINT_FORBIDDEN"),
				"{endpoint}: {message}"
			);
			assert!(message.contains("permitted_private_origins"), "{message}");
		}
	}

	#[rstest]
	fn a_host_name_needs_https_unless_its_origin_is_permitted(
		#[values(http, mcp)] transport: fn(&str) -> Value,
	) {
		let message =
			refusal(validation().validate_tool(&transport("http://tools.example.com/rpc")));
		assert!(message.contains("TOOL_ENDPOINT_FORBIDDEN"), "{message}");
		assert!(message.contains("HTTPS"), "{message}");
		permitting(&["http://tools.example.com"])
			.validate_tool(&transport("http://tools.example.com/rpc"))
			.unwrap();
	}

	#[rstest]
	fn a_permitted_origin_admits_exactly_that_origin(
		#[values(http, mcp)] transport: fn(&str) -> Value,
	) {
		let validation = permitting(&["http://127.0.0.1:8080"]);
		validation
			.validate_tool(&transport("http://127.0.0.1:8080/rpc"))
			.unwrap();
		for other in [
			"http://127.0.0.1:9090/rpc",
			"https://127.0.0.1:8080/rpc",
			"http://127.0.0.2:8080/rpc",
		] {
			let message = refusal(validation.validate_tool(&transport(other)));
			assert!(
				message.contains("TOOL_ENDPOINT_FORBIDDEN"),
				"{other}: {message}"
			);
		}
	}

	#[rstest]
	fn origins_permitted_after_the_validation_was_built_still_count() {
		let origins = KeylessAccess::default();
		let validation = validation().with_keyless_origins(origins.clone());
		assert!(
			validation
				.validate_tool(&http("http://127.0.0.1:7000/rpc"))
				.is_err()
		);
		origins
			.permit_origin_of("http://127.0.0.1:7000/anything")
			.unwrap();
		validation
			.validate_tool(&http("http://127.0.0.1:7000/rpc"))
			.unwrap();
	}

	#[rstest]
	fn native_tools_have_no_endpoint_to_check() {
		validation()
			.validate_tool(&json!({"transport": "native", "operation": "echo"}))
			.unwrap();
	}

	#[rstest]
	fn registration_refuses_a_tool_definition_with_a_private_endpoint() {
		let mut tool = crate::test_support::http_tool(NODE, "private-tool", "private_tool");
		tool.config["transport"]["endpoint"] = json!("http://10.1.2.3/rpc");
		let message = refusal(validation().validate_in(&tool, true));
		assert!(message.contains("TOOL_ENDPOINT_FORBIDDEN"), "{message}");
		tool.config["transport"]["endpoint"] = json!("https://production.example/rpc");
		validation().validate_in(&tool, true).unwrap();
	}

	fn agent_with_server(name: &str, url: &str) -> Entry {
		let stored = crate::test_support::stored_plugin("acme-tools", &[("lint", "Lint it.")]);
		let mut reference = crate::test_support::plugin_reference(&stored);
		reference.servers.insert(
			ServerName::try_from(name.to_owned()).unwrap(),
			McpServer::Remote(RemoteServer {
				url: url.parse().unwrap(),
				headers: vec![],
			}),
		);
		let mut agent = crate::test_support::agent("agent");
		agent.config["agent_plugins"] = json!([reference]);
		agent
	}

	#[rstest]
	fn an_agent_takes_in_the_remote_servers_of_a_plugin_at_public_addresses() {
		validation()
			.validate_in(&crate::test_support::agent("agent"), true)
			.unwrap();
		validation()
			.validate_in(
				&agent_with_server("search", "https://search.example.com/mcp?v=1"),
				true,
			)
			.unwrap();
	}

	#[rstest]
	fn an_agent_is_refused_for_a_plugin_server_at_a_private_address() {
		for url in [
			"http://127.0.0.1:8080/mcp",
			"https://10.0.0.5/mcp?token=abc",
			"https://[::1]/mcp",
		] {
			let message =
				refusal(validation().validate_in(&agent_with_server("private", url), true));
			assert!(
				message.contains("AGENT_PLUGIN_SERVER_FORBIDDEN: acme-tools/private"),
				"{url}: {message}"
			);
		}
	}

	#[rstest]
	fn a_plugin_server_needs_https_unless_its_origin_is_permitted() {
		let agent = agent_with_server("local", "http://localhost:8080/mcp");
		let message = refusal(validation().validate_in(&agent, true));
		assert!(
			message.contains("AGENT_PLUGIN_SERVER_FORBIDDEN: acme-tools/local"),
			"{message}"
		);
		assert!(message.contains("HTTPS"), "{message}");
		permitting(&["http://localhost:8080"])
			.validate_in(&agent, true)
			.unwrap();
	}

	#[rstest]
	fn a_permitted_origin_admits_a_plugin_server_at_a_private_address() {
		let agent = agent_with_server("private", "http://127.0.0.1:8080/mcp");
		permitting(&["http://127.0.0.1:8080"])
			.validate_in(&agent, true)
			.unwrap();
	}
}
