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
fn hostless_endpoint_always_requires_approval() {
	let valid = model();
	let mut hostless = valid.clone();
	hostless.config["endpoint"] = json!("file:///tmp/model");
	for (old, new) in [
		(&valid, &hostless),
		(&hostless, &valid),
		(&hostless, &hostless),
	] {
		assert_eq!(compare(old, new), Ok(vec!["endpoint host".to_owned()]));
	}
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
		("scopes", json!(null), "scopes"),
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
		hook: Hook::Approval,
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

fn agent_binding(
	kind: &str,
	id: &str,
	version: &str,
	extra: serde_json::Value,
) -> serde_json::Value {
	let mut binding = json!({"kind":kind,"target":{"registry_deployment":"aidash://local","id":id,"version":version}});
	for (key, value) in extra.as_object().unwrap() {
		binding[key] = value.clone();
	}
	binding
}

fn agent_with(bindings: serde_json::Value) -> Entry {
	entry(
		"agent",
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"first","bindings":bindings}),
	)
}

fn restricted(old: serde_json::Value, new: serde_json::Value) -> Vec<String> {
	compare(&agent_with(json!([old])), &agent_with(json!([new]))).unwrap()
}

#[test]
fn widened_binding_hosts_scope_and_limits_require_approval_and_narrowing_carries() {
	let fetch = |version: &str, narrow: serde_json::Value| {
		agent_binding("tool", "fetch", version, json!({ "narrow": narrow }))
	};
	let base = json!({"allowed_hosts":["docs.a.com"],"scope":{"recipient":["alice"]},"limits":{"max_bytes":10}});
	for wider in [
		json!({"scope":{"recipient":["alice"]},"limits":{"max_bytes":10}}),
		json!({"allowed_hosts":["docs.a.com","b.example"],"scope":{"recipient":["alice"]},"limits":{"max_bytes":10}}),
		json!({"allowed_hosts":["docs.a.com"],"limits":{"max_bytes":10}}),
		json!({"allowed_hosts":["docs.a.com"],"scope":{"recipient":["alice","bob"]},"limits":{"max_bytes":10}}),
		json!({"allowed_hosts":["docs.a.com"],"scope":{"recipient":["alice"]}}),
		json!({"allowed_hosts":["docs.a.com"],"scope":{"recipient":["alice"]},"limits":{"max_bytes":20}}),
		json!({"allowed_hosts":["docs.a.com"],"scope":{"recipient":["alice"]},"limits":{"max_bytes":10},"provider_tool":{"max_tool_calls":3,"parameters":{}}}),
	] {
		assert!(
			restricted(fetch("1.0.0", base.clone()), fetch("1.1.0", wider.clone()))
				.contains(&"binding restrictions".to_owned()),
			"{wider}"
		);
	}
	for narrower in [
		base.clone(),
		json!({"allowed_hosts":["docs.a.com"],"scope":{"recipient":["alice"],"path":["notes"]},"limits":{"max_bytes":5}}),
	] {
		assert!(
			restricted(
				fetch("1.0.0", base.clone()),
				fetch("2.0.0", narrower.clone())
			)
			.is_empty(),
			"{narrower}"
		);
	}
	assert!(restricted(fetch("1.0.0", json!({})), fetch("1.0.0", base)).is_empty());
}

#[test]
fn dropping_an_explicit_restriction_on_a_default_builtin_requires_approval() {
	let file_read = agent_binding(
		"tool",
		"aidash.file_read",
		"1.0.0",
		json!({"narrow":{"scope":{"path":["notes"]}}}),
	);
	let old = agent_with(json!([file_read]));
	let new = agent_with(json!([]));
	assert_eq!(
		compare(&old, &new).unwrap(),
		vec!["binding restrictions".to_owned()]
	);
	assert!(compare(&new, &old).unwrap().is_empty());
}

#[test]
fn bundle_members_may_narrow_but_not_widen() {
	let kit = |members: serde_json::Value| {
		agent_binding("bundle", "kit", "1.0.0", json!({ "members": members }))
	};
	for wider in [json!([]), json!(["read", "write"]), json!(["write"])] {
		assert!(
			restricted(kit(json!(["read"])), kit(wider.clone()))
				.contains(&"binding restrictions".to_owned()),
			"{wider}"
		);
	}
	assert!(restricted(kit(json!(["read", "write"])), kit(json!(["read"]))).is_empty());
	assert!(restricted(kit(json!([])), kit(json!(["read"]))).is_empty());
}

#[test]
fn decider_binding_restrictions_may_tighten_but_not_loosen() {
	let probability = |value: f64| {
		serde_json::to_value(crate::decision::Probability::new(value).unwrap()).unwrap()
	};
	let decider = |decision: Option<serde_json::Value>| {
		let narrow = decision.map_or_else(|| json!({}), |decision| json!({ "decision": decision }));
		agent_binding("decider", "guard", "1.0.0", json!({ "narrow": narrow }))
	};
	let strict = json!({"threshold":probability(0.3),"preserve_recent":8,"forbid_apply":true,"max_calls_per_run":5});
	for looser in [
		None,
		Some(
			json!({"threshold":probability(0.4),"preserve_recent":8,"forbid_apply":true,"max_calls_per_run":5}),
		),
		Some(
			json!({"threshold":probability(0.3),"preserve_recent":6,"forbid_apply":true,"max_calls_per_run":5}),
		),
		Some(
			json!({"threshold":probability(0.3),"preserve_recent":8,"forbid_apply":false,"max_calls_per_run":5}),
		),
		Some(
			json!({"threshold":probability(0.3),"preserve_recent":8,"forbid_apply":true,"max_calls_per_run":null}),
		),
	] {
		assert!(
			restricted(decider(Some(strict.clone())), decider(looser.clone()))
				.contains(&"binding restrictions".to_owned()),
			"{looser:?}"
		);
	}
	let stricter = json!({"threshold":probability(0.2),"preserve_recent":10,"forbid_apply":true,"max_calls_per_run":2});
	assert!(restricted(decider(Some(strict)), decider(Some(stricter))).is_empty());
}

#[test]
fn personal_secret_ownership_cannot_widen_to_any() {
	for transport in [
		json!({"transport":"http","endpoint":"https://example.com/a","replay":"unsafe","secret":"gh","secret_owner":"personal"}),
		json!({"transport":"mcp","endpoint":"https://example.com/a","tool_name":"search","replay":"unsafe","idempotency_argument":null,"secret":"gh","secret_owner":"personal"}),
	] {
		let personal = entry("tool", transport);
		let mut any = personal.clone();
		any.config.as_object_mut().unwrap().remove("secret_owner");
		assert_eq!(
			compare(&personal, &any).unwrap(),
			vec!["secret owner".to_owned()]
		);
		assert!(compare(&any, &personal).unwrap().is_empty());
	}
}

#[test]
fn a_changed_operation_that_stays_read_only_requires_approval() {
	let http = entry(
		"tool",
		json!({"transport":"http","endpoint":"https://example.com/lookup","replay":"read_only"}),
	);
	let mut changed = http.clone();
	changed.config["endpoint"] = json!("https://example.com/delete");
	assert_eq!(
		compare(&http, &changed).unwrap(),
		vec!["read-only operation".to_owned()]
	);
	let mcp = entry(
		"tool",
		json!({"transport":"mcp","endpoint":"https://example.com/mcp","tool_name":"search","replay":"read_only","idempotency_argument":null}),
	);
	for (field, value) in [
		("tool_name", json!("delete")),
		("endpoint", json!("https://example.com/other")),
	] {
		let mut changed = mcp.clone();
		changed.config[field] = value;
		assert_eq!(
			compare(&mcp, &changed).unwrap(),
			vec!["read-only operation".to_owned()],
			"{field}"
		);
	}
	// Without a Read-only declaration, a path or operation change still carries.
	let mut unsafe_mcp = mcp.clone();
	unsafe_mcp.config["replay"] = json!("unsafe");
	let mut renamed = unsafe_mcp.clone();
	renamed.config["tool_name"] = json!("delete");
	assert!(compare(&unsafe_mcp, &renamed).unwrap().is_empty());
}

#[test]
fn a_decider_reaching_jev_or_another_credential_requires_approval() {
	use crate::decision::{
		AnswerType, DeciderConfig, DeciderRoute, Hook, Mode, policy::StateRetention,
	};
	let mut decisions = DeciderRoute::jev("openai/gpt-oss");
	decisions.provider_contract = crate::decision::DECISIONS.into();
	decisions.hosts = vec!["cloudflare".into()];
	let config = DeciderConfig {
		hook: Hook::Approval,
		answer_type: AnswerType::Noul,
		description: "x".into(),
		provider_credential: "openrouter".into(),
		routes: vec![decisions],
		builder: "x".into(),
		option_source: "x".into(),
		rule: "x".into(),
		mode: Mode::Enforce,
		max_calls_per_run: 20,
		state_retention: StateRetention::default(),
	};
	let old = entry("decider", serde_json::to_value(&config).unwrap());
	let mut jev = config.clone();
	jev.routes.push(DeciderRoute::jev("typesafe/jev-1.13"));
	assert_eq!(
		compare(&old, &entry("decider", serde_json::to_value(&jev).unwrap())).unwrap(),
		vec!["route hosts".to_owned()]
	);
	let mut credential = config.clone();
	credential.provider_credential = "other".into();
	assert_eq!(
		compare(
			&old,
			&entry("decider", serde_json::to_value(&credential).unwrap())
		)
		.unwrap(),
		vec!["provider credential".to_owned()]
	);
}

#[test]
fn a_tool_descriptor_restriction_or_operation_change_requires_approval() {
	let descriptor = entry(
		"tool",
		json!({"registry_deployment":"aidash://local","provider":"integration.http@1","operation":"invoke","default_alias":"lookup","tier":"integration","narrow":{"allowed_hosts":["docs.a.com"]},"transport":{"transport":"http","endpoint":"https://example.com/rpc","replay":"unsafe"}}),
	);
	let mut wider = descriptor.clone();
	wider.config["narrow"] = json!({});
	assert_eq!(
		compare(&descriptor, &wider).unwrap(),
		vec!["restrictions".to_owned()]
	);
	let mut operation = descriptor.clone();
	operation.config["operation"] = json!("other");
	assert_eq!(
		compare(&descriptor, &operation).unwrap(),
		vec!["tool operation".to_owned()]
	);
	let mut alias = descriptor.clone();
	alias.config["default_alias"] = json!("renamed");
	alias.config["narrow"] = json!({"allowed_hosts":["docs.a.com"],"limits":{"max":1}});
	assert!(compare(&descriptor, &alias).unwrap().is_empty());
}
