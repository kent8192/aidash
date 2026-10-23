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
