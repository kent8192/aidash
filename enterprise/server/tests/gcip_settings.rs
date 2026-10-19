//! The managed GCIP settings that Cloud Environments write reach the
//! Enterprise sign-in, which refuses invalid ones before connecting to anything.
use std::process::Command;

#[test]
fn managed_gcip_settings_are_validated_at_startup() {
	// Arrange: an unreachable database proves the refusal comes first.
	let settings = tempfile::tempdir().unwrap();
	let base = include_str!("../../../server/settings/base.example.toml")
		.replace(
			"[core]\n",
			"[core]\nsecret_key = 'startup-fixture-secret-key-0123456789'\n",
		)
		.replace(
			"[node]\n",
			"[node]\napi_token = 'startup-fixture-deployment-administrator-0123456789'\n",
		)
		.replace("offering = \"self-hosted\"", "offering = \"cloud\"");
	std::fs::write(settings.path().join("base.toml"), base).unwrap();
	let managed = settings.path().join("gcip.json");
	std::fs::write(
		&managed,
		serde_json::to_vec(&serde_json::json!({"dashboard": {"gcip": {
			"project_id": "fixture-project",
			"web_api_key": "public-fixture-key",
			"public_origin": "http://dashboard.example.test",
			"tenant_bindings": {"Pool-X": "acme"}
		}}}))
		.unwrap(),
	)
	.unwrap();
	// Act
	let output = Command::new(env!("CARGO_BIN_EXE_aidash"))
		.arg("serve")
		.env_clear()
		.env("REINHARDT_ENV", "local")
		.env("REINHARDT_SETTINGS_DIR", settings.path())
		.env("AIDASH_GCIP_SETTINGS", &managed)
		.env("DATABASE_URL", "postgres://aidash@127.0.0.1:1/unreachable")
		.output()
		.unwrap();
	// Assert
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(
		stderr.contains("invalid dashboard.gcip.public_origin"),
		"{stderr}"
	);
}
