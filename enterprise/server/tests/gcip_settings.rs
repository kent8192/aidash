// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

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
			"[deployment]\n",
			"[deployment]\napi_token = 'startup-fixture-deployment-administrator-0123456789'\n",
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

#[rstest::rstest]
#[case::common(
	true,
	"dashboard.common_auth cannot be combined with the build's own sign-in"
)]
#[case::direct_oidc(
	false,
	"dashboard.oidc cannot be combined with the build's own sign-in"
)]
fn managed_direct_gcip_cannot_be_combined_with_another_sign_in(
	#[case] common: bool,
	#[case] refusal: &str,
) {
	// A real Enterprise child process installs its Build and reads managed
	// scoped GCIP settings. Neither process-global test replacement nor a DB is needed.
	let directory = tempfile::tempdir().unwrap();
	let base = include_str!("../../../server/settings/base.example.toml")
		.replace(
			"[core]\n",
			"[core]\nsecret_key = 'mixed-sign-in-fixture-secret-0123456789'\n",
		)
		.replace(
			"[deployment]\n",
			"[deployment]\napi_token = 'mixed-sign-in-fixture-deployment-administrator'\n",
		)
		.replace("offering = \"self-hosted\"", "offering = \"cloud\"");
	let mut source: toml::Value = toml::from_str(&base).unwrap();
	let mut dashboard = serde_json::json!({"oidc": {
		"issuer":"https://authentication.example.test", "client_id":"cloud",
		"client_secret":"mixed-sign-in-fixture-client-secret", "public_origin":"https://cloud.example.test"
	}});
	if common {
		// These deliberately absent files prove mixed configuration is rejected
		// before attempting to load common-auth credentials or connect anywhere.
		dashboard["common_auth"] = serde_json::json!({
			"grpc_endpoint":"https://127.0.0.1:1", "server_domain":"authentication.example.test",
			"ca_certificate_file":directory.path().join("missing-ca.pem"),
			"client_certificate_file":directory.path().join("missing-client.pem"),
			"client_private_key_file":directory.path().join("missing-key.pem"),
			"source_issuers":{"https://securetoken.google.com/fixture-project/tenants/shared":""}
		});
	}
	source.as_table_mut().unwrap().insert(
		"dashboard".into(),
		toml::Value::try_from(dashboard).unwrap(),
	);
	std::fs::write(
		directory.path().join("base.toml"),
		toml::to_string(&source).unwrap(),
	)
	.unwrap();
	let managed = directory.path().join("managed-gcip.json");
	std::fs::write(
		&managed,
		serde_json::to_vec(&serde_json::json!({"dashboard":{"gcip":{
			"project_id":"fixture-project", "web_api_key":"public-fixture-key",
			"public_origin":"https://cloud.example.test", "shared_pool":"shared"
		}}}))
		.unwrap(),
	)
	.unwrap();
	let output = Command::new(env!("CARGO_BIN_EXE_aidash"))
		.arg("serve")
		.env_clear()
		.env("REINHARDT_ENV", "local")
		.env("REINHARDT_SETTINGS_DIR", directory.path())
		.env("AIDASH_GCIP_SETTINGS", managed)
		.env("DATABASE_URL", "postgres://fixture@127.0.0.1:1/unreachable")
		.output()
		.unwrap();
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(stderr.contains(refusal), "{stderr}");
	assert!(!stderr.contains("mixed-sign-in-fixture-client-secret"));
}
