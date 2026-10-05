//! Existing deployment variables participate in Reinhardt's settings composition.
use indexmap::IndexMap;
use reinhardt::conf::settings::sources::{ConfigSource, ScopedSource, SourceError};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub struct LegacyEnvironment;

impl ConfigSource for LegacyEnvironment {
	fn load(&self) -> Result<IndexMap<String, Value>, SourceError> {
		values(|name| std::env::var(name).ok())
	}

	fn load_scoped(&self) -> Result<ScopedSource, SourceError> {
		Ok(ScopedSource {
			values: self.load()?,
			interpolation_file: None,
		})
	}

	fn priority(&self) -> u8 {
		55
	}

	fn description(&self) -> String {
		"Aidash deployment environment".into()
	}
}

fn values(read: impl Fn(&str) -> Option<String>) -> Result<IndexMap<String, Value>, SourceError> {
	let mut node = serde_json::Map::new();
	for (key, name) in [
		("node_id", "AIDASH_NODE_ID"),
		("endpoint", "AIDASH_ENDPOINT"),
		("api_token", "AIDASH_API_TOKEN"),
		("nats_url", "NATS_URL"),
		("worker_count", "AIDASH_WORKER_SLOTS"),
		("probe_listen", "AIDASH_PROBE_LISTEN"),
	] {
		if let Some(value) = read(name) {
			node.insert(key.into(), Value::String(value));
		}
	}
	if let Some(value) = read("AIDASH_WEB_DIR") {
		let path = std::path::PathBuf::from(value);
		// The legacy CLI resolves this variable relative to its working directory.
		let path = if path.is_absolute() {
			path
		} else {
			std::env::current_dir()?.join(path)
		};
		node.insert("web_dir".into(), json!(path));
	}
	let mut core = serde_json::Map::new();
	if let Some(token) = read("AIDASH_API_TOKEN") {
		// Native command validation requires a framework key even though Aidash
		// session/CSRF protocols keep their own keys. Derive a stable separate key.
		core.insert(
			"secret_key".into(),
			json!(format!(
				"{:x}",
				Sha256::digest(format!("aidash-framework:{token}"))
			)),
		);
	}
	if let Some(url) = read("DATABASE_URL") {
		core.insert("databases".into(), json!({"default": database(&url)?}));
	}
	let mut oidc = serde_json::Map::new();
	for (key, name) in [
		("issuer", "AIDASH_OIDC_ISSUER"),
		("client_id", "AIDASH_OIDC_CLIENT_ID"),
		("client_secret", "AIDASH_OIDC_CLIENT_SECRET"),
		("public_origin", "AIDASH_OIDC_PUBLIC_ORIGIN"),
		("keycloak_admin_url", "AIDASH_OIDC_KEYCLOAK_ADMIN_URL"),
		("status_client_id", "AIDASH_OIDC_STATUS_CLIENT_ID"),
		("status_client_secret", "AIDASH_OIDC_STATUS_CLIENT_SECRET"),
		(
			"session_absolute_seconds",
			"AIDASH_OIDC_SESSION_ABSOLUTE_SECONDS",
		),
		("session_idle_seconds", "AIDASH_OIDC_SESSION_IDLE_SECONDS"),
	] {
		if let Some(value) = read(name) {
			oidc.insert(key.into(), Value::String(value));
		}
	}
	let mut kubernetes = serde_json::Map::new();
	for (key, name) in [
		("namespace", "AIDASH_KUBERNETES_NAMESPACE"),
		("release", "AIDASH_KUBERNETES_RELEASE"),
	] {
		if let Some(value) = read(name) {
			kubernetes.insert(key.into(), Value::String(value));
		}
	}
	if !kubernetes.is_empty() {
		// Helm exports these only on servers with observations enabled. Keep
		// partial configuration visible so settings validation rejects it.
		kubernetes.insert("enabled".into(), Value::Bool(true));
	}
	let mut result = IndexMap::new();
	if !node.is_empty() {
		result.insert("node".into(), Value::Object(node));
	}
	if !core.is_empty() {
		result.insert("core".into(), Value::Object(core));
	}
	if !kubernetes.is_empty() {
		result.insert("kubernetes".into(), Value::Object(kubernetes));
	}
	if !oidc.is_empty() {
		result.insert("dashboard".into(), json!({"oidc": oidc}));
	}
	Ok(result)
}

fn database(value: &str) -> Result<Value, SourceError> {
	let invalid = || SourceError::Parse("DATABASE_URL must be a PostgreSQL URL".into());
	let url = reqwest::Url::parse(value).map_err(|_| invalid())?;
	if !matches!(url.scheme(), "postgres" | "postgresql") || url.fragment().is_some() {
		return Err(invalid());
	}
	let decode = |value: &str| {
		percent_encoding::percent_decode_str(value)
			.decode_utf8()
			.map(|value| value.into_owned())
			.map_err(|_| invalid())
	};
	let options: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
	Ok(json!({
		"engine": "postgresql",
		"name": url.path().strip_prefix('/').unwrap_or(url.path()),
		"user": if url.username().is_empty() { None } else { Some(decode(url.username())?) },
		"password": url.password().map(decode).transpose()?,
		"host": url.host_str(),
		"port": url.port(),
		"options": options,
	}))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[rstest::rstest]
	fn deployment_variables_preserve_database_credentials_and_options() {
		let data = values(|name| match name {
            "DATABASE_URL" => Some("postgres://user:p%40ss@localhost:5433/aidash?sslmode=require&application_name=worker".into()),
            "AIDASH_NODE_ID" => Some("aidash://test".into()),
            "AIDASH_WORKER_SLOTS" => Some("2".into()),
            _ => None,
        }).unwrap();
		let db = &data["core"]["databases"]["default"];
		assert_eq!(db["password"], "p@ss");
		assert_eq!(db["port"], 5433);
		assert_eq!(db["options"]["sslmode"], "require");
		assert_eq!(db["options"]["application_name"], "worker");
		assert_eq!(data["node"]["worker_count"], "2");
		assert!(!data.contains_key("dashboard"));
	}

	#[rstest::rstest]
	#[case::helm(Some("aidash"), Some("production"), true)]
	#[case::namespace_only(Some("aidash"), None, true)]
	#[case::release_only(None, Some("production"), true)]
	#[case::disabled(None, None, false)]
	fn helm_observations_participate_in_settings_composition(
		#[case] namespace: Option<&str>,
		#[case] release: Option<&str>,
		#[case] enabled: bool,
	) {
		// Arrange: use the variables rendered by the server Helm workload.
		let data = values(|name| match name {
			"AIDASH_KUBERNETES_NAMESPACE" => namespace.map(str::to_owned),
			"AIDASH_KUBERNETES_RELEASE" => release.map(str::to_owned),
			_ => None,
		})
		.unwrap();
		// Act: deserialize the same fragment consumed by composed settings.
		let settings: crate::apps::operations::serializers::settings::KubernetesSettings =
			serde_json::from_value(data.get("kubernetes").cloned().unwrap_or(json!({}))).unwrap();
		// Assert: absent Helm variables preserve the disabled default; partial
		// configuration remains enabled and is rejected by fragment validation.
		assert_eq!(settings.enabled, enabled);
		assert_eq!(settings.namespace, namespace.unwrap_or_default());
		assert_eq!(settings.release, release.unwrap_or_default());
		use reinhardt::conf::settings::{fragment::SettingsValidation, profile::Profile};
		assert_eq!(
			settings.validate(&Profile::Development).is_ok(),
			!enabled || (namespace.is_some() && release.is_some())
		);
	}

	#[rstest::rstest]
	fn invalid_database_urls_do_not_expose_credentials() {
		let error = database("https://secret:password@example.test/database").unwrap_err();
		assert_eq!(
			error.to_string(),
			"Parse error: DATABASE_URL must be a PostgreSQL URL"
		);
	}
}
