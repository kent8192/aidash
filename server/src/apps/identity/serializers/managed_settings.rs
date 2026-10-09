//! Managed Cloud GCIP settings composed through Reinhardt's source interface.
use super::settings::GcipConfig;
use indexmap::IndexMap;
use reinhardt::conf::settings::sources::{ConfigSource, ScopedSource, SourceError};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

pub(crate) struct ManagedGcipSource {
	pub(crate) path: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
	dashboard: Dashboard,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dashboard {
	gcip: GcipConfig,
}

impl ManagedGcipSource {
	pub(crate) fn from_env() -> Self {
		Self {
			path: std::env::var_os("AIDASH_GCIP_SETTINGS").map(PathBuf::from),
		}
	}
}

impl ConfigSource for ManagedGcipSource {
	fn load(&self) -> Result<IndexMap<String, Value>, SourceError> {
		let Some(path) = &self.path else {
			return Ok(IndexMap::new());
		};
		let envelope: Envelope = serde_json::from_slice(&std::fs::read(path)?)?;
		Ok(IndexMap::from([(
			"dashboard".into(),
			json!({"gcip": envelope.dashboard.gcip}),
		)]))
	}

	fn load_scoped(&self) -> Result<ScopedSource, SourceError> {
		Ok(ScopedSource {
			values: self.load()?,
			interpolation_file: None,
		})
	}

	fn priority(&self) -> u8 {
		56 // Managed settings override files without replacing higher-priority environment settings.
	}

	fn description(&self) -> String {
		"Managed Cloud GCIP settings".into()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[rstest::rstest]
	#[case::other_section(json!({"dashboard": {"gcip": {}}, "node": {"api_token": "override"}}))]
	#[case::other_issuer(json!({"dashboard": {"gcip": {}, "oidc": {}}}))]
	#[case::missing_file(Value::Null)]
	fn invalid_managed_source_is_not_silently_ignored(#[case] mut value: Value) {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("settings.json");
		if !value.is_null() {
			value["dashboard"]["gcip"] = json!({
				"project_id": "fixture-project",
				"web_api_key": "public",
				"public_origin": "https://test.aidash.run",
				"tenant_bindings": {"Pool-X": "acme"}
			});
			std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
		}
		let source = ManagedGcipSource { path: Some(path) };
		assert!(source.load_scoped().is_err());
	}
}
