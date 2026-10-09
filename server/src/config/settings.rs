//! Settings module for server
//!
//! This module provides environment-specific settings configuration using TOML files.
//!
//! ## Configuration Structure
//!
//! Settings are loaded from TOML files in the `settings/` directory:
//! - `base.toml` - Common settings across all environments
//! - `local.toml` - Local development settings
//! - `staging.toml` - Staging environment settings
//! - `production.toml` - Production environment settings
//!
//! ## Priority Order
//!
//! Settings are merged with the following priority (highest to lowest):
//! 1. Environment variables with `REINHARDT_` prefix
//! 2. Managed non-secret Provider Credential JSON, when configured
//! 3. Environment-specific TOML file (e.g., `production.toml`)
//! 4. Base TOML file (`base.toml`)
//! 5. Default values
//!
//! GCP startup supplies `AIDASH_PROVIDER_CREDENTIAL_SETTINGS` as the path to a
//! read-only descriptor file containing only the Provider Credential section.
//! The fingerprint key stays in its referenced environment variable.
//!
//! ## Environment Selection
//!
//! The environment is determined by the `REINHARDT_ENV` environment variable:
//! - `local` or `development` → loads `local.toml`
//! - `staging` → loads `staging.toml`
//! - `production` → loads `production.toml`
//!
//! If `REINHARDT_ENV` is not set, it defaults to `local`.
//!
//! ## Environment Variable Interpolation
//!
//! `TomlFileSource` interpolates `${VAR}` syntax inside TOML string values
//! by default (since reinhardt-web v0.1.0-rc.27). The `${...}` syntax is
//! not valid in non-string TOML literals. Supported forms:
//!
//! - `${VAR}` — required; settings load fails if `VAR` is unset
//! - `${VAR:-default}` — falls back to `default` when `VAR` is unset
//! - `${VAR:?message}` — settings load fails with `message` when `VAR` is unset
//!
//! Interpolated strings are typed-coerced at deserialization time, so
//! `pool_size = "${DB_POOL_SIZE:-10}"` resolves directly to the field's
//! declared Rust type (e.g. `u16`) without manual parsing.

use crate::apps::federation::remote::serializers::settings::NodeSettings;
use crate::apps::identity::serializers::managed_settings::ManagedGcipSource;
use crate::apps::identity::serializers::provider_credentials::ManagedSource;
use crate::apps::identity::serializers::provider_credentials::Settings as ProviderCredentialSettings;
use crate::apps::identity::serializers::settings::DashboardSettings;
use crate::apps::operations::serializers::settings::KubernetesSettings;
use reinhardt::conf::settings::PendingSettings;
use reinhardt::conf::settings::builder::{BuildError, SettingsBuilder};
use reinhardt::conf::settings::profile::Profile;
use reinhardt::conf::settings::scoped::ScopedSettings;
use reinhardt::conf::settings::sources::{DefaultSource, HighPriorityEnvSource, TomlFileSource};
use reinhardt::settings;
use std::env;

// Add fragments to extend settings: e.g. `#[settings(core: CoreSettings | cache: CacheSettings)]`
#[settings(core: CoreSettings | contacts: ContactSettings | migrations: MigrationSettings | node: NodeSettings | dashboard: DashboardSettings | kubernetes: KubernetesSettings | provider_credentials: ProviderCredentialSettings)]
pub struct ProjectSettings;

/// Get settings based on environment variable
///
/// Reads the REINHARDT_ENV environment variable to determine which settings to load.
/// Defaults to "local" if not set.
///
/// # Examples
///
/// ```no_run
/// use aidash_server::config::settings::get_settings;
///
/// let settings = get_settings().expect("settings sources should load");
/// ```
///
/// # Errors
///
/// Returns an error when a settings source cannot be loaded or parsed.
pub fn get_settings() -> Result<PendingSettings<ProjectSettings>, BuildError> {
	settings_builder()?.build_pending_composed::<ProjectSettings>()
}

/// Merge settings without expanding unselected runtime secrets.
pub fn get_scoped_settings() -> Result<ScopedSettings, BuildError> {
	settings_builder()?.build_scoped()
}

fn settings_builder() -> Result<SettingsBuilder, BuildError> {
	let profile_str = env::var("REINHARDT_ENV").unwrap_or_else(|_| "local".to_string());

	// Resolve the managed project root independently of the caller's working directory.
	let base_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
	let settings_dir = env::var_os("REINHARDT_SETTINGS_DIR")
		.map(std::path::PathBuf::from)
		.unwrap_or_else(|| base_dir.join("settings"));
	let managed = env::var_os("AIDASH_PROVIDER_CREDENTIAL_SETTINGS").map(std::path::PathBuf::from);

	// Presence inspection does not interpolate unselected runtime credentials.
	let configured =
		managed_settings_builder(&profile_str, &base_dir, &settings_dir, managed.as_deref())
			.add_source(ManagedGcipSource::from_env())
			.add_source(HighPriorityEnvSource::new().with_prefix("REINHARDT_"))
			.build_scoped()?
			.has_path(&["dashboard", "oidc"]);
	Ok(
		managed_settings_builder(&profile_str, &base_dir, &settings_dir, managed.as_deref())
			.add_source(ManagedGcipSource::from_env())
			.add_source(super::legacy_env::LegacyEnvironment::new(configured))
			.add_source(HighPriorityEnvSource::new().with_prefix("REINHARDT_")),
	)
}

fn managed_settings_builder(
	profile: &str,
	base_dir: &std::path::Path,
	settings_dir: &std::path::Path,
	managed: Option<&std::path::Path>,
) -> SettingsBuilder {
	let builder = file_settings_builder(profile, base_dir, settings_dir);
	if let Some(path) = managed {
		builder.add_source(ManagedSource::new(path))
	} else {
		builder
	}
}

fn file_settings_builder(
	profile_name: &str,
	base_dir: &std::path::Path,
	settings_dir: &std::path::Path,
) -> SettingsBuilder {
	let file_profile = match profile_name {
		"development" => "local",
		name => name,
	};
	// Build settings by merging sources in priority order.
	// The composed and scoped paths use deep merging, so a
	// single key in `production.toml` overrides only that key — sibling
	// entries inside the same nested table inherit from `base.toml`.
	SettingsBuilder::new()
		.profile(Profile::parse(profile_name))
        // Lowest priority: Default values
        .add_source(
            DefaultSource::new()
                .with_value("core", serde_json::json!({ "base_dir": base_dir, "installed_apps": super::apps::APP_LABELS }))
                // Initialize the standard REST scaffold contact defaults.
                .with_value("contacts", serde_json::json!({}))
                // Initialize optional Aidash fragments without enabling OIDC or Kubernetes.
                .with_value("dashboard", serde_json::json!({}))
                .with_value("kubernetes", serde_json::json!({}))
                .with_value("provider_credentials", serde_json::json!({}))
                .with_value("migrations", serde_json::json!({})),
        )
        // Medium priority: Base TOML file
        .add_source(TomlFileSource::new(settings_dir.join("base.toml")))
        // Profile priority: Environment-specific TOML file
        .add_source(TomlFileSource::new(
			settings_dir.join(format!("{file_profile}.toml")),
		))
}

/// Return plain project settings for consumers whose evaluator type is `ProjectSettings`.
pub fn get_shell_settings() -> ProjectSettings {
	get_settings()
		.expect("Failed to build settings")
		.resolve()
		.expect("Failed to resolve settings")
		.into_parts()
		.0
}

use async_trait::async_trait;
use reinhardt::{DiError, DiResult, Injectable, InjectionContext};
#[async_trait]
impl Injectable for ProjectSettings {
	async fn inject(context: &InjectionContext) -> DiResult<Self> {
		context
			.get_singleton::<Self>()
			.map(|settings| (*settings).clone())
			.ok_or_else(|| DiError::NotFound("Aidash project settings".into()))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use reinhardt::conf::settings::fragment::SettingsValidation;

	fn managed_fixture() -> tempfile::TempDir {
		let directory = tempfile::tempdir().unwrap();
		let source = include_str!("../../settings/base.example.toml")
			.replace(
				"[core]\n",
				"[core]\nsecret_key = \"isolated-settings-test-secret-0123456789\"\n",
			)
			.replace(
				"[node]\n",
				"[node]\napi_token = \"isolated-settings-test-operator-0123456789\"\n",
			);
		std::fs::write(directory.path().join("base.toml"), source).unwrap();
		directory
	}

	fn managed_descriptor() -> serde_json::Value {
		serde_json::json!({"provider_credentials": {
			"store": {"byok_project_id":"aidash-byok-fixture", "environment_id":"test", "fingerprint_env":"AIDASH_SECRET_PROVIDER_FINGERPRINT"},
			"broker": {"endpoint":"https://broker.run.app/api/v1", "issuer":"aidash", "audience":"test", "kid":"projects/fixture/locations/us-central1/keyRings/capability/cryptoKeys/capability/cryptoKeyVersions/1"}
		}})
	}

	#[rstest::rstest]
	fn managed_provider_settings_reach_composed_startup_and_disable_cleanly() {
		// Arrange: the same descriptor and source path emitted by host bootstrap.
		let directory = managed_fixture();
		let path = directory.path().join("managed.json");
		std::fs::write(&path, managed_descriptor().to_string()).unwrap();
		let build = || {
			managed_settings_builder("local", directory.path(), directory.path(), Some(&path))
				.build_pending_composed::<ProjectSettings>()
				.unwrap()
				.resolve()
				.unwrap()
		};
		// Act: exercise Reinhardt's actual composition and typed validation.
		let resolved = build();
		let settings = &resolved.settings().provider_credentials;
		settings.validate(&Profile::parse("local")).unwrap();
		// Assert: Store and broker both reach worker composition without any Key Material.
		assert_eq!(settings.store.as_ref().unwrap().environment_id, "test");
		assert_eq!(
			settings.store.as_ref().unwrap().fingerprint_env,
			"AIDASH_SECRET_PROVIDER_FINGERPRINT"
		);
		assert_eq!(
			settings.broker.as_ref().unwrap().endpoint,
			"https://broker.run.app/api/v1"
		);
		assert_eq!(settings.broker.as_ref().unwrap().audience, "test");
		std::fs::write(directory.path().join("local.toml"), format!("[provider_credentials.store]\nbyok_project_id = 'aidash-byok-fixture'\nenvironment_id = 'test'\nfingerprint_env = 'AIDASH_SECRET_PROVIDER_FINGERPRINT'\n[provider_credentials.broker]\nendpoint = 'https://fallback.run.app/api/v1'\nissuer = 'aidash'\naudience = 'test'\nkid = '{}'\n", settings.broker.as_ref().unwrap().kid)).unwrap();
		std::fs::write(
			&path,
			r#"{"provider_credentials":{"store":null,"broker":null}}"#,
		)
		.unwrap();
		let disabled = build();
		assert!(disabled.settings().provider_credentials.store.is_none());
		assert!(disabled.settings().provider_credentials.broker.is_none());
	}

	#[rstest::rstest]
	fn managed_provider_settings_fail_closed_and_do_not_expand_other_secrets() {
		let directory = managed_fixture();
		let path = directory.path().join("managed.json");
		let mut invalid = managed_descriptor();
		invalid["provider_credentials"]["broker"]["audience"] = serde_json::json!("other");
		for value in [
			invalid,
			serde_json::json!({"provider_credentials": {}, "core": {"debug": true}}),
		] {
			std::fs::write(&path, value.to_string()).unwrap();
			let result =
				managed_settings_builder("local", directory.path(), directory.path(), Some(&path))
					.build_pending_composed::<ProjectSettings>()
					.and_then(|pending| pending.resolve());
			assert!(match result {
				Ok(resolved) => resolved
					.settings()
					.provider_credentials
					.validate(&Profile::parse("local"))
					.is_err(),
				Err(_) => true,
			});
		}
		std::fs::remove_file(&path).unwrap();
		assert!(
			managed_settings_builder("local", directory.path(), directory.path(), Some(&path))
				.build_scoped()
				.is_err()
		);
		std::fs::write(&path, managed_descriptor().to_string()).unwrap();
		std::fs::write(
			directory.path().join("local.toml"),
			"[dashboard.oidc]\nclient_secret = '${AIDASH_UNSELECTED_OIDC_REGRESSION_SECRET}'\n",
		)
		.unwrap();
		let scoped =
			managed_settings_builder("local", directory.path(), directory.path(), Some(&path))
				.build_scoped()
				.unwrap();
		assert!(scoped.has_path(&["provider_credentials", "broker"]));
		assert!(scoped.has_path(&["dashboard", "oidc"]));
	}

	#[rstest::rstest]
	#[case::enabled(true)]
	#[case::disabled(false)]
	fn managed_provider_store_settings_are_loaded_without_secret_material(#[case] enabled: bool) {
		let directory = tempfile::tempdir().unwrap();
		let source = include_str!("../../settings/base.example.toml")
			.replace(
				"[core]\n",
				"[core]\nsecret_key = 'isolated-settings-test-secret-0123456789'\n",
			)
			.replace(
				"[node]\n",
				"[node]\napi_token = 'isolated-settings-test-operator-0123456789'\n",
			);
		std::fs::write(directory.path().join("base.toml"), source).unwrap();
		let path = directory.path().join("settings.json");
		let store = enabled.then(|| {
			serde_json::json!({
				"byok_project_id":"aidash-byok-fixture","environment_id":"pr-42",
				"fingerprint_env":"AIDASH_SECRET_PROVIDER_FINGERPRINT"
			})
		});
		std::fs::write(
			&path,
			serde_json::to_vec(
				&serde_json::json!({"provider_credentials":{"store":store,"broker":null}}),
			)
			.unwrap(),
		)
		.unwrap();
		let resolved =
			managed_settings_builder("local", directory.path(), directory.path(), Some(&path))
				.build_pending_composed::<ProjectSettings>()
				.unwrap()
				.resolve()
				.unwrap();
		let config = &resolved.settings().provider_credentials.store;
		if enabled {
			let config = config.as_ref().unwrap();
			assert_eq!(config.byok_project_id, "aidash-byok-fixture");
			assert_eq!(config.environment_id, "pr-42");
			assert_eq!(config.fingerprint_env, "AIDASH_SECRET_PROVIDER_FINGERPRINT");
			assert_eq!(config.max_per_tenant, 20);
		} else {
			assert!(config.is_none());
		}
	}

	#[rstest::rstest]
	fn managed_provider_settings_reject_other_sections_and_missing_sources() {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("settings.json");
		assert!(
			managed_settings_builder("local", directory.path(), directory.path(), Some(&path))
				.build_scoped()
				.is_err()
		);
		std::fs::write(
			&path,
			r#"{"provider_credentials":{"store":null},"core":{"secret_key":"forbidden"}}"#,
		)
		.unwrap();
		assert!(
			managed_settings_builder("local", directory.path(), directory.path(), Some(&path))
				.build_scoped()
				.is_err()
		);
	}

	#[rstest::rstest]
	#[case::local("local")]
	#[case::development_alias("development")]
	fn profile_overrides_preserve_required_base_settings(#[case] profile: &str) {
		// Arrange: isolated files supply only local test credentials.
		let directory = tempfile::tempdir().unwrap();
		let source = include_str!("../../settings/base.example.toml")
			.replace(
				"[core]\n",
				"[core]\nsecret_key = \"isolated-settings-test-secret-0123456789\"\n",
			)
			.replace(
				"[node]\n",
				"[node]\napi_token = \"isolated-settings-test-operator-0123456789\"\n",
			);
		std::fs::write(directory.path().join("base.toml"), source).unwrap();
		std::fs::write(
			directory.path().join("local.toml"),
			"[core]\ndebug = true\n[node]\nworker_count = 2\n",
		)
		.unwrap();
		std::fs::write(
			directory.path().join("development.toml"),
			"[node]\nworker_count = 9\n",
		)
		.unwrap();
		// Act: exercise the same file composition used by process startup.
		let settings = file_settings_builder(profile, directory.path(), directory.path())
			.build_pending_composed::<ProjectSettings>()
			.unwrap()
			.resolve()
			.unwrap();
		// Assert: an override keeps sibling values and the installed app registry.
		assert_eq!(
			settings.settings().core.secret_key,
			"isolated-settings-test-secret-0123456789"
		);
		assert!(settings.settings().core.debug);
		assert_eq!(settings.settings().node.worker_count, 2);
		assert_eq!(settings.settings().node.node_id, "aidash://local");
		assert!(settings.settings().contacts.admins.is_empty());
		assert!(settings.settings().contacts.managers.is_empty());
		assert!(settings.settings().dashboard.oidc.is_none());
		assert!(!settings.settings().kubernetes.enabled);
		assert_eq!(
			settings.settings().core.installed_apps,
			super::super::apps::APP_LABELS
		);
	}

	#[rstest::rstest]
	fn oidc_presence_does_not_resolve_unselected_credentials() {
		// Arrange: management operations may only need to inspect provider presence.
		let directory = tempfile::tempdir().unwrap();
		std::fs::write(
			directory.path().join("base.toml"),
			concat!(
				"[dashboard.oidc]\n",
				"client_id = 'configured-client'\n",
				"client_secret = '${AIDASH_UNSELECTED_OIDC_REGRESSION_SECRET}'\n",
			),
		)
		.unwrap();
		// Act: inspect the same scoped settings graph before adding legacy tuning.
		let settings = file_settings_builder("local", directory.path(), directory.path())
			.build_scoped()
			.unwrap();
		// Assert: no interpolation or required-field validation is needed for presence.
		assert!(settings.has_path(&["dashboard", "oidc"]));
	}

	#[rstest::rstest]
	fn managed_gcip_settings_preserve_tenant_ids_and_apply_typed_defaults() {
		let directory = tempfile::tempdir().unwrap();
		let base = include_str!("../../settings/base.example.toml")
			.replace(
				"[core]\n",
				"[core]\nsecret_key = 'isolated-settings-test-secret-0123456789'\n",
			)
			.replace(
				"[node]\n",
				"[node]\napi_token = 'isolated-settings-test-operator-0123456789'\n",
			);
		std::fs::write(directory.path().join("base.toml"), base).unwrap();
		let path = directory.path().join("dashboard.json");
		std::fs::write(
			&path,
			serde_json::to_vec(&serde_json::json!({"dashboard": {"gcip": {
				"project_id": "fixture-project",
				"public_origin": "https://test.aidash.run",
				"web_api_key": "public-fixture-key",
				"tenant_bindings": {"Pool-X": "acme"},
				"providers": {"Pool-X": ["google.com"]},
				"password_sign_up": []
			}}}))
			.unwrap(),
		)
		.unwrap();
		let settings = file_settings_builder("local", directory.path(), directory.path())
			.add_source(ManagedGcipSource { path: Some(path) })
			.build_pending_composed::<ProjectSettings>()
			.unwrap()
			.resolve()
			.unwrap();
		let dashboard = &settings.settings().dashboard;
		assert!(dashboard.oidc.is_none());
		let gcip = dashboard.gcip.as_ref().unwrap();
		assert_eq!(
			gcip.issuer(),
			"https://securetoken.google.com/fixture-project"
		);
		assert_eq!(gcip.tenant_bindings["Pool-X"], "acme");
		assert_eq!(gcip.providers["Pool-X"], ["google.com"]);
		assert!(gcip.password_sign_up.is_empty());
		assert_eq!(gcip.session_absolute_seconds, 43200);
		assert_eq!(gcip.session_idle_seconds, 1800);
	}
}
