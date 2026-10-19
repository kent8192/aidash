//! The Community `aidash` refuses the Cloud and Dedicated Offerings before it
//! connects to anything, and the Enterprise build's sign-in.
use rstest::rstest;
use std::process::Command;

#[rstest]
#[case("cloud")]
#[case("dedicated")]
fn the_community_build_refuses_other_offerings(#[case] offering: &str) {
	// Arrange: an unreachable database proves the refusal comes first.
	let settings = tempfile::tempdir().unwrap();
	std::fs::write(
		settings.path().join("base.toml"),
		format!("[deployment]\noffering = \"{offering}\"\n"),
	)
	.unwrap();
	// Act
	let output = Command::new(env!("CARGO_BIN_EXE_aidash"))
		.arg("serve")
		.env_clear()
		.env("REINHARDT_ENV", "local")
		.env("REINHARDT_SETTINGS_DIR", settings.path())
		.env("DATABASE_URL", "postgres://aidash@127.0.0.1:1/unreachable")
		.output()
		.unwrap();
	// Assert
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(
		stderr.contains(&format!(
			"the Community build does not run the {offering} Offering; \
			 set [deployment] offering to \"self-hosted\""
		)),
		"{stderr}"
	);
}

#[test]
fn the_community_build_refuses_gcip_sign_in() {
	let settings = tempfile::tempdir().unwrap();
	let base = include_str!("../../../../settings/base.example.toml")
		.replace(
			"[core]\n",
			"[core]\nsecret_key = 'startup-fixture-secret-key-0123456789'\n",
		)
		.replace(
			"[node]\n",
			"[node]\napi_token = 'startup-fixture-deployment-administrator-0123456789'\n",
		);
	std::fs::write(
		settings.path().join("base.toml"),
		format!("{base}\n[dashboard.gcip]\nproject_id = \"fixture-project\"\n"),
	)
	.unwrap();
	let output = Command::new(env!("CARGO_BIN_EXE_aidash"))
		.arg("serve")
		.env_clear()
		.env("REINHARDT_ENV", "local")
		.env("REINHARDT_SETTINGS_DIR", settings.path())
		.env("DATABASE_URL", "postgres://aidash@127.0.0.1:1/unreachable")
		.output()
		.unwrap();
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(
		stderr.contains(
			"the Community build has no GCIP sign-in; [dashboard.gcip] needs the Enterprise build"
		),
		"{stderr}"
	);
}
