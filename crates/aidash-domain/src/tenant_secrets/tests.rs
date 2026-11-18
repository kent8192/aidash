use super::*;
use crate::tool::ToolConfig;

#[test]
fn names_are_lowercase_identifiers() {
	for good in ["search", "brave-key", "a_1", "0abc"] {
		assert!(validate_name(good).is_ok(), "{good}");
	}
	for bad in [
		"",
		"Search",
		"-lead",
		"_lead",
		"has space",
		"a.b",
		&"x".repeat(65),
	] {
		assert!(validate_name(bad).is_err(), "{bad}");
	}
}

#[test]
fn hosts_are_bare_normalized_and_deduplicated() {
	assert_eq!(
		normalize_hosts(&[
			"API.Example.com".into(),
			"api.example.com".into(),
			"b.example".into()
		])
		.unwrap(),
		["api.example.com", "b.example"]
	);
	for bad in [
		"https://a.example",
		"a.example/path",
		"a.example:8443",
		"*.example",
		"user@a.example",
		"",
	] {
		assert!(normalize_host(bad).is_err(), "{bad}");
	}
	assert!(normalize_hosts(&[]).is_err());
	let many: Vec<String> = (0..=MAX_HOSTS).map(|i| format!("h{i}.example")).collect();
	assert!(normalize_hosts(&many).is_err());
}

#[test]
fn a_secret_goes_only_to_a_recorded_https_host() {
	let hosts = vec!["api.example.com".to_owned()];
	assert!(permits(&hosts, "https://API.example.com/v1/tool").unwrap());
	assert!(permits(&hosts, "https://api.example.com:8443/v1").unwrap());
	assert!(!permits(&hosts, "https://evil.example/v1").unwrap());
	assert!(!permits(&hosts, "https://sub.api.example.com/").unwrap());
	// Never in plaintext, except to a literal loopback address.
	assert!(permits(&hosts, "http://api.example.com/").is_err());
	assert!(permits(&["127.0.0.1".to_owned()], "http://127.0.0.1:9/").unwrap());
}

#[test]
fn a_tool_names_its_secret_and_endpoint() {
	let http: ToolConfig = serde_json::from_value(serde_json::json!({
		"transport": "http", "endpoint": "https://api.example.com/run",
		"replay": "safe", "secret": "search"
	}))
	.unwrap();
	assert_eq!(
		named(&http),
		Some(("search", "https://api.example.com/run"))
	);
	let plain: ToolConfig = serde_json::from_value(serde_json::json!({
		"transport": "http", "endpoint": "https://api.example.com/run", "replay": "safe"
	}))
	.unwrap();
	assert_eq!(named(&plain), None);
}

#[test]
fn a_personal_only_definition_has_no_shared_target_in_either_shape() {
	let entry = |config: serde_json::Value| -> crate::registry::Entry {
		serde_json::from_value(serde_json::json!({
			"id":"t","version":"1.0.0","kind":"tool",
			"name":{"en":"t"},"description":{"en":"t"},"config":config
		}))
		.unwrap()
	};
	let legacy = |owner: Option<&str>| {
		let mut config = serde_json::json!({
			"transport":"http","endpoint":"https://api.example.com/run",
			"replay":"safe","secret":"search"
		});
		if let Some(owner) = owner {
			config["secret_owner"] = owner.into();
		}
		entry(config)
	};
	let descriptor = |owner: Option<&str>| {
		let mut transport = serde_json::json!({
			"transport":"mcp","endpoint":"https://api.example.com/mcp","tool_name":"x",
			"replay":"safe","secret":"search"
		});
		if let Some(owner) = owner {
			transport["secret_owner"] = owner.into();
		}
		entry(serde_json::json!({"provider":"integration.mcp@1","transport":transport}))
	};
	for (shared, personal) in [
		(legacy(None), legacy(Some("personal"))),
		(descriptor(None), descriptor(Some("personal"))),
	] {
		assert!(!definition_personal_only(&shared));
		assert!(definition_shared_target(&shared).is_some());
		assert_eq!(definition_secret(&shared).as_deref(), Some("search"));
		assert!(definition_personal_only(&personal));
		assert_eq!(definition_shared_target(&personal), None);
		// The name stays available for validation and Marketplace slot mapping.
		assert_eq!(definition_secret(&personal).as_deref(), Some("search"));
	}
	assert!(!definition_personal_only(&entry(
		serde_json::json!({"x":1})
	)));
}

fn secret(id: u128, owner: Option<&str>) -> TenantSecret {
	TenantSecret {
		id: Uuid::from_u128(id),
		tenant: "acme".into(),
		name: "search".into(),
		hosts: vec!["api.example.com".into()],
		owner: owner.map(str::to_owned),
		secret_resource: format!("aidash-store/tenant-secrets/{id}"),
		pinned_version: Some("v1".into()),
		fingerprint: "fingerprint".into(),
		last4: "cdef".into(),
		state: State::Active,
		created_at: Utc::now(),
		updated_at: Utc::now(),
		revision: 1,
	}
}

#[test]
fn a_personal_secret_wins_and_may_fall_back_only_when_any_owner_is_allowed() {
	let personal = secret(1, Some("alice"));
	let shared = secret(2, None);
	let owner = crate::tool::SecretOwner::Any;
	assert_eq!(
		pick_secret(Some(&personal), Some(&shared), owner).map(|secret| secret.id),
		Some(personal.id)
	);
	assert_eq!(
		pick_secret(None, Some(&shared), owner).map(|secret| secret.id),
		Some(shared.id)
	);
	assert!(pick_secret(None, Some(&shared), crate::tool::SecretOwner::Personal).is_none());
	assert!(pick_secret(None, None, owner).is_none());
}

fn agent_with_server_secrets(settings: serde_json::Value) -> crate::registry::Entry {
	let plugin = crate::agent_plugin::fixtures::reference_with_servers(
		"acme-tools",
		serde_json::json!({
			"docs": {"type": "streamable-http", "url": "https://docs.example.com/mcp"},
			"wiki": {"type": "streamable-http", "url": "https://wiki.example.com/mcp"},
		}),
	);
	serde_json::from_value(serde_json::json!({
		"id":"agent","version":"1.0.0","kind":"agent",
		"name":{"en":"agent"},"description":{"en":"agent"},
		"config":{
			"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work",
			"agent_plugins":[plugin],"agent_plugin_settings":{"acme-tools":settings}
		}
	}))
	.unwrap()
}

#[test]
fn an_agent_names_the_shared_capable_secrets_of_its_plugin_servers_for_secret_use() {
	let agent = agent_with_server_secrets(serde_json::json!({
		"docs": {"secret": {"name": "docs-token"}},
		"wiki": {"secret": {"name": "wiki-token", "owner": "personal"}},
	}));

	// A personal-only server secret names only each person's own row.
	assert_eq!(definition_secret_names(&agent), ["docs-token"]);
	assert_eq!(
		plugin_shared_targets(&agent),
		[(
			"docs-token".to_owned(),
			"https://docs.example.com/mcp".to_owned()
		)]
	);
	// An Agent has no Tool secret of its own.
	assert_eq!(definition_secret(&agent), None);
}

#[test]
fn the_same_secret_on_two_servers_is_named_once() {
	let agent = agent_with_server_secrets(serde_json::json!({
		"docs": {"secret": {"name": "shared-token"}},
		"wiki": {"secret": {"name": "shared-token"}},
	}));

	assert_eq!(definition_secret_names(&agent), ["shared-token"]);
}
