use super::*;
use serde_json::json;

#[test]
fn an_old_transport_defaults_to_a_shared_secret_without_declared_scopes() {
	for config in [
		json!({
			"transport": "http", "endpoint": "https://api.example.com/run",
			"replay": "safe", "secret": "search"
		}),
		json!({
			"transport": "mcp", "endpoint": "https://mcp.example.com",
			"tool_name": "lookup", "replay": "safe", "idempotency_argument": null,
			"secret": "search"
		}),
	] {
		let parsed: ToolConfig = serde_json::from_value(config.clone()).unwrap();
		let owner = match &parsed {
			ToolConfig::Http { secret_owner, .. } | ToolConfig::Mcp { secret_owner, .. } => {
				*secret_owner
			}
			_ => panic!("not an external transport"),
		};
		assert_eq!(owner, SecretOwner::Any, "{config}");
		if let ToolConfig::Mcp { scopes, .. } = &parsed {
			assert_eq!(*scopes, None, "{config}");
		}
		assert_eq!(serde_json::to_value(&parsed).unwrap(), config);
	}
}

#[test]
fn a_personal_owner_and_declared_scopes_round_trip() {
	let config = json!({
		"transport": "mcp", "endpoint": "https://mcp.example.com",
		"tool_name": "lookup", "replay": "safe", "idempotency_argument": null,
		"secret": "search", "scopes": ["files:read"], "secret_owner": "personal"
	});
	let parsed: ToolConfig = serde_json::from_value(config.clone()).unwrap();
	let ToolConfig::Mcp {
		secret_owner,
		scopes,
		..
	} = &parsed
	else {
		panic!("not an MCP transport");
	};
	assert_eq!(*secret_owner, SecretOwner::Personal);
	assert_eq!(*scopes, Some(vec!["files:read".to_owned()]));
	assert_eq!(serde_json::to_value(&parsed).unwrap(), config);
}

#[test]
fn any_is_the_default_and_the_only_skipped_owner() {
	assert!(SecretOwner::default().is_any());
	assert!(!SecretOwner::Personal.is_any());
	assert_eq!(
		serde_json::to_value(SecretOwner::Any).unwrap(),
		json!("any")
	);
	assert_eq!(
		serde_json::to_value(SecretOwner::Personal).unwrap(),
		json!("personal")
	);
}
