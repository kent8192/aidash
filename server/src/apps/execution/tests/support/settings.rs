use aidash_server::config::settings::ProjectSettings;
use reinhardt::conf::settings::{builder::SettingsBuilder, sources::DefaultSource};
use serde_json::json;

/// Isolated settings use only values belonging to this disposable fixture.
pub fn settings_for(database_url: &str) -> ProjectSettings {
	let url = reqwest::Url::parse(database_url).unwrap();
	SettingsBuilder::new()
		.add_source(
			DefaultSource::new()
				.with_value(
					"core",
					json!({
						"secret_key":"endpoint-settings-fixture-only",
						"base_dir":env!("CARGO_MANIFEST_DIR"),
						"databases":{"default":{
							"engine":"postgresql", "name":url.path().trim_start_matches('/'),
							"host":url.host_str().unwrap(), "port":url.port().unwrap(),
							"user":url.username(), "password":url.password().unwrap_or_default()
						}}
					}),
				)
				.with_value("contacts", json!({}))
				.with_value("migrations", json!({}))
				.with_value(
					"node",
					json!({
						"node_id":"aidash://endpoint-test", "endpoint":"http://localhost",
						"api_token":"operator-endpoint-fixture", "background_enabled":false
					}),
				)
				.with_value("dashboard", json!({}))
				.with_value("provider_credentials", json!({}))
				.with_value("kubernetes", json!({})),
		)
		.build_composed::<ProjectSettings>()
		.expect("valid native endpoint settings")
}

/// Serialize only disposable fixture settings for child processes. Reinhardt
/// deliberately redacts SecretString during ordinary serialization; restore
/// these local database passwords explicitly inside the RAII-owned directory.
#[allow(dead_code)] // Only process integration targets write settings files.
pub fn process_settings(settings: &ProjectSettings) -> String {
	let mut source = toml::Value::try_from(settings).unwrap();
	for (name, database) in &settings.core.databases {
		if let Some(password) = &database.password {
			source["core"]["databases"][name]["password"] =
				toml::Value::String(password.expose_secret().to_owned());
		}
	}
	toml::to_string(&source).unwrap()
}
