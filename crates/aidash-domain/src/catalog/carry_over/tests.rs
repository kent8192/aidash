use super::*;
use serde_json::json;

fn model() -> Entry {
	serde_json::from_value(json!({"id":"example","version":"1.0.0","kind":"model","name":{},"description":{},"config":{"provider":"p","model_id":"m","endpoint":"https://example.com/v1","context_window":100,"modalities":[],"cost":{}}})).unwrap()
}

#[test]
fn model_host_change_requires_approval() {
	let old = model();
	let mut new = old.clone();
	new.config["endpoint"] = json!("https://other.example/v1");
	assert_eq!(compare(&old, &new), Ok(vec!["endpoint host".to_owned()]));
}

#[test]
fn model_consent_fields_and_ignored_fields() {
	let old = model();
	for (field, value, expected) in [
		("provider", json!("other"), "provider"),
		(
			"provider_credential",
			json!("credential"),
			"provider credential",
		),
		("plan_connection", json!("plan"), "plan connection"),
	] {
		let mut new = old.clone();
		new.config[field] = value;
		assert!(
			compare(&old, &new).unwrap().contains(&expected.to_owned()),
			"{field}"
		);
	}
	let mut new = old.clone();
	new.config["endpoint"] = json!("https://example.com/elsewhere");
	new.config["model_id"] = json!("new");
	new.config["modalities"] = json!(["image"]);
	new.config["wire_api"] = json!("chat");
	assert_eq!(compare(&old, &new).unwrap(), Vec::<String>::new());
}

fn entry(kind: &str, config: serde_json::Value) -> Entry {
	serde_json::from_value(
		json!({"id":"example","version":"1.0.0","kind":kind,"name":{},"description":{},"config":config}),
	)
	.unwrap()
}

#[test]
fn tool_consent_detects_host_secret_scopes_and_new_read_only() {
	let old = entry(
		"tool",
		json!({"transport":"mcp","endpoint":"https://example.com/a","tool_name":"search","replay":"unsafe","idempotency_argument":null,"secret":"one","scopes":["read"]}),
	);
	for (field, value, expected) in [
		(
			"endpoint",
			json!("https://other.example/a"),
			"endpoint host",
		),
		("secret", json!("two"), "secret"),
		("scopes", json!(["read", "write"]), "scopes"),
		("replay", json!("read_only"), "read-only operation"),
	] {
		let mut new = old.clone();
		new.config[field] = value;
		assert!(
			compare(&old, &new).unwrap().contains(&expected.to_owned()),
			"{field}"
		);
	}
	let mut new = old.clone();
	new.config["endpoint"] = json!("https://example.com/b");
	new.config["scopes"] = json!([]);
	assert!(compare(&old, &new).unwrap().is_empty());
	let mut read_only = old.clone();
	read_only.config["replay"] = json!("read_only");
	assert!(compare(&read_only, &old).unwrap().is_empty());
}

#[test]
fn agent_dependency_versions_do_not_change_consent_but_identifiers_do() {
	let old = entry(
		"agent",
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"first"}),
	);
	let mut new = old.clone();
	new.config["model"]["version"] = json!("2.0.0");
	new.config["instructions"] = json!("different");
	new.tags.push("new".into());
	assert!(compare(&old, &new).unwrap().is_empty());
	new.config["model"]["id"] = json!("another");
	assert!(
		compare(&old, &new)
			.unwrap()
			.contains(&"dependency".to_owned())
	);
}

#[test]
fn decider_host_and_retention_expansion_require_approval() {
	use crate::decision::{
		AnswerType, DeciderConfig, DeciderRoute, Hook, Mode, policy::StateRetention,
	};
	let config = DeciderConfig {
		hook: Hook::Compaction,
		answer_type: AnswerType::Noul,
		description: "x".into(),
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.13")],
		builder: "x".into(),
		option_source: "x".into(),
		rule: "x".into(),
		mode: Mode::Enforce,
		max_calls_per_run: 20,
		state_retention: StateRetention::default(),
	};
	let old = entry("decider", serde_json::to_value(config).unwrap());
	let mut new = old.clone();
	new.config["routes"][0]["hosts"] = json!(["new-host"]);
	assert!(
		compare(&old, &new)
			.unwrap()
			.contains(&"route hosts".to_owned())
	);
	new = old.clone();
	new.config["state_retention"]["enabled"] = json!(true);
	assert!(
		compare(&old, &new)
			.unwrap()
			.contains(&"state retention".to_owned())
	);
	let mut retained = new.clone();
	retained.config["state_retention"]["days"] = json!(8);
	assert!(
		compare(&new, &retained)
			.unwrap()
			.contains(&"state retention".to_owned())
	);
	let mut harmless = old.clone();
	harmless.config["routes"][0]["model"] = json!("typesafe/jev-1.14");
	harmless.config["max_calls_per_run"] = json!(40);
	harmless.config["routes"][0]["threshold"] =
		serde_json::to_value(crate::decision::Probability::new(0.4).unwrap()).unwrap();
	assert!(compare(&old, &harmless).unwrap().is_empty());
	assert!(compare(&retained, &new).unwrap().is_empty());
}

#[test]
fn agent_default_switch_requires_approval() {
	let old = entry(
		"agent",
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"first"}),
	);
	let mut new = old.clone();
	new.config["remove_default"] = json!(["task_create"]);
	assert!(
		compare(&old, &new)
			.unwrap()
			.contains(&"remove default".to_owned())
	);
}

#[test]
fn agent_plugin_addition_requires_approval_and_removal_does_not() {
	let old = entry(
		"agent",
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"first"}),
	);
	let mut new = old.clone();
	new.config["agent_plugins"] = json!([{ "digest": format!("sha256:{}", "a".repeat(64)), "name":"plugin", "source":"https://github.com/example/plugin", "skills":[], "servers":{}, "findings":[] }]);
	assert!(
		compare(&old, &new)
			.unwrap()
			.contains(&"agent plugins".to_owned())
	);
	assert!(compare(&new, &old).unwrap().is_empty());
}

#[test]
fn http_tool_host_secret_and_new_read_only_require_approval() {
	let old = entry(
		"tool",
		json!({"transport":"http","endpoint":"https://example.com/a","replay":"unsafe","secret":"one"}),
	);
	for (field, value, expected) in [
		(
			"endpoint",
			json!("https://another.example/a"),
			"endpoint host",
		),
		("secret", json!("two"), "secret"),
		("replay", json!("read_only"), "read-only operation"),
	] {
		let mut changed = old.clone();
		changed.config[field] = value;
		assert!(
			compare(&old, &changed)
				.unwrap()
				.contains(&expected.to_owned())
		);
	}
	let mut changed = old.clone();
	changed.config["endpoint"] = json!("https://example.com/b");
	changed.schema = json!({"new":"schema"});
	assert!(compare(&old, &changed).unwrap().is_empty());
}

#[test]
fn agent_plugin_settings_can_narrow_but_not_add_tools_or_read_only_declarations() {
	let plugin = crate::agent_plugin::fixtures::reference_with_servers(
		"example",
		json!({"docs":{"type":"streamable-http","url":"https://example.com/mcp"}}),
	);
	let mut old = entry(
		"agent",
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"first","agent_plugins":[plugin],"agent_plugin_settings":{"example":{"docs":{"tools":["search","read"]}}}}),
	);
	let mut new = old.clone();
	new.config["agent_plugin_settings"]["example"]["docs"]["tools"] = json!(["search"]);
	assert!(compare(&old, &new).unwrap().is_empty());
	assert!(
		compare(&new, &old)
			.unwrap()
			.contains(&"agent plugin settings".to_owned())
	);
	old.config["agent_plugin_settings"]["example"]["docs"]["read_only"] =
		json!([{"name":"search","schema_digest":format!("sha256:{}", "a".repeat(64))}]);
	assert!(
		compare(&new, &old)
			.unwrap()
			.contains(&"agent plugin settings".to_owned())
	);
}

#[test]
fn restored_default_capabilities_and_allow_flags_change_consent() {
	let old = entry(
		"agent",
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"first","remove_default":["file_search","file_read","task_create"]}),
	);
	let mut new = old.clone();
	new.config["remove_default"] = json!([]);
	let changes = compare(&old, &new).unwrap();
	assert!(changes.contains(&"core capabilities".to_owned()));
	assert!(changes.contains(&"allow flags".to_owned()));
	assert!(changes.contains(&"remove default".to_owned()));
	let narrowing = compare(&new, &old).unwrap();
	assert!(!narrowing.contains(&"core capabilities".to_owned()));
	assert!(!narrowing.contains(&"allow flags".to_owned()));
	assert!(narrowing.contains(&"remove default".to_owned()));
}

#[test]
fn source_is_the_highest_lower_carrying_enabled_semver() {
	let candidates = [
		CarryCandidate {
			version: "2.1.0",
			enabled: true,
			carry_over: true,
		},
		CarryCandidate {
			version: "2.0.0",
			enabled: true,
			carry_over: true,
		},
		CarryCandidate {
			version: "2.0.1-alpha",
			enabled: true,
			carry_over: false,
		},
		CarryCandidate {
			version: "2.0.1-rc.1",
			enabled: false,
			carry_over: true,
		},
		CarryCandidate {
			version: "1.9.0",
			enabled: true,
			carry_over: true,
		},
	];
	assert_eq!(source_version("2.0.1", &candidates).unwrap(), Some("2.0.0"));
	assert_eq!(source_version("1.0.0", &candidates).unwrap(), None);
	let build = [CarryCandidate {
		version: "2.0.1+first",
		enabled: true,
		carry_over: true,
	}];
	assert_eq!(source_version("2.0.1+second", &build).unwrap(), None);
	assert!(source_version("invalid", &candidates).is_err());
}
