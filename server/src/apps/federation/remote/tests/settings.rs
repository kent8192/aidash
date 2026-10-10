use super::*;
use crate::config::Config;
use reinhardt::conf::settings::sources::DefaultSource;
use rstest::{fixture, rstest};
use serde_json::{Value, json};

#[fixture]
fn settings_source() -> Value {
	json!({
		"core":{"secret_key":"test-settings-key", "databases":{"default":{
			"engine":"postgresql", "name":"fixture", "host":"localhost",
			"user":"fixture", "password":"fixture", "port":5432
		}}},
		"contacts":{}, "migrations":{}, "dashboard":{}, "kubernetes":{}, "provider_credentials":{}, "cache_salt":{},
		"node":{"node_id":"aidash://test-settings", "endpoint":"http://localhost",
			"api_token":"test-only-operator-secret"}
	})
}

fn build(value: Value) -> Result<ProjectSettings, BuildError> {
	let mut source = DefaultSource::new();
	for (key, value) in value.as_object().unwrap() {
		source = source.with_value(key, value.clone());
	}
	let settings = SettingsBuilder::new()
		.add_source(source)
		.build_composed::<ProjectSettings>()?;
	settings
		.validate(&Profile::parse("local"))
		.map_err(|error| BuildError::Validation(error.to_string()))?;
	Ok(settings)
}

#[rstest]
fn settings_defaults_and_runtime_share_the_native_database(settings_source: Value) {
	// Arrange
	let settings = build(settings_source).unwrap();
	// Act
	let runtime = Config::from_settings(&settings).unwrap();
	// Assert
	assert_eq!(runtime.node_id, "aidash://test-settings");
	assert_eq!(
		runtime.database_url,
		"postgresql://fixture:fixture@localhost:5432/fixture"
	);
	assert_eq!(runtime.lease_seconds, 30);
	assert_eq!(settings.node.worker_count, 4);
	assert!(runtime.oidc.is_none());
	assert!(settings.provider_credentials.store.is_none());
	assert!(!format!("{:?}", settings.node).contains("test-only-operator-secret"));
}

#[rstest]
#[case("api_token", json!("short"))]
#[case("node_id", json!("wrong://node"))]
#[case("endpoint", json!("file:///etc/passwd"))]
#[case("lease_seconds", json!(2))]
#[case("worker_count", json!(65))]
#[case("nats_url", json!("file:///tmp/nats"))]
#[case("probe_listen", json!("not-an-address"))]
fn invalid_node_settings_fail_before_startup(
	mut settings_source: Value,
	#[case] field: &str,
	#[case] invalid: Value,
) {
	// Arrange
	settings_source["node"][field] = invalid;
	// Act
	let result = build(settings_source);
	// Assert
	assert!(result.is_err());
}

#[fixture]
fn oidc_settings(mut settings_source: Value) -> Value {
	settings_source["dashboard"]["oidc"] = json!({
		"issuer":"https://identity.example/realms/test", "client_id":"dashboard",
		"client_secret":"test-only-client-secret", "public_origin":"https://example.com:443/",
		"keycloak_admin_url":"https://identity.example/admin/", "status_client_id":"status",
		"status_client_secret":"test-only-status-secret"
	});
	settings_source
}

#[rstest]
fn optional_oidc_is_normalized_and_secrets_are_redacted(oidc_settings: Value) {
	// Arrange
	let settings = build(oidc_settings).unwrap();
	// Act
	let oidc = Config::from_settings(&settings).unwrap().oidc.unwrap();
	// Assert
	assert_eq!(oidc.public_origin, "https://example.com");
	assert_eq!(oidc.keycloak_admin_url, "https://identity.example/admin");
	assert_eq!(oidc.session_idle_seconds, 1800);
	let debug = format!("{oidc:?}");
	assert!(!debug.contains("test-only-client-secret"));
	assert!(!debug.contains("test-only-status-secret"));
}

#[rstest]
#[case("issuer", json!("http://identity.example/realms/test"))]
#[case("public_origin", json!("https://example.com/path"))]
#[case("session_idle_seconds", json!(604800))]
fn invalid_oidc_settings_fail_before_startup(
	mut oidc_settings: Value,
	#[case] field: &str,
	#[case] invalid: Value,
) {
	// Arrange
	oidc_settings["dashboard"]["oidc"][field] = invalid;
	// Act
	let result = build(oidc_settings);
	// Assert
	assert!(result.is_err());
}
