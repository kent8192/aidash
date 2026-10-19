//! The Community `aidash` refuses the Cloud and Dedicated Offerings before it
//! connects to anything.
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
