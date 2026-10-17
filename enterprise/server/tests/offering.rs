//! The Enterprise `aidash` runs only the Cloud and Dedicated Offerings.
use std::process::{Command, Output};

/// Run `aidash serve` with only `base.toml` as its settings, and no database.
fn serve(name: &str, base: &str) -> Output {
	let settings = std::env::temp_dir().join(format!(
		"aidash-enterprise-offering-{}-{name}",
		std::process::id()
	));
	std::fs::create_dir_all(&settings).unwrap();
	std::fs::write(settings.join("base.toml"), base).unwrap();
	let output = Command::new(env!("CARGO_BIN_EXE_aidash"))
		.arg("serve")
		.env_clear()
		.env("REINHARDT_ENV", "local")
		.env("REINHARDT_SETTINGS_DIR", &settings)
		.output()
		.unwrap();
	std::fs::remove_dir_all(&settings).unwrap();
	output
}

const REFUSAL: &str = "the Enterprise build does not run the self-hosted Offering; \
	set [deployment] offering to \"cloud\" or \"dedicated\"";

#[test]
fn the_default_offering_is_refused() {
	let output = serve("default", "");
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(stderr.contains(REFUSAL), "{stderr}");
}

#[test]
fn the_self_hosted_offering_is_refused() {
	let output = serve("self-hosted", "[deployment]\noffering = \"self-hosted\"\n");
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(!output.status.success());
	assert!(stderr.contains(REFUSAL), "{stderr}");
}

#[test]
fn the_cloud_and_dedicated_offerings_pass_the_build_check() {
	for offering in ["cloud", "dedicated"] {
		let output = serve(
			offering,
			&format!("[deployment]\noffering = \"{offering}\"\n"),
		);
		let stderr = String::from_utf8_lossy(&output.stderr);
		assert!(!stderr.contains("does not run"), "{offering}: {stderr}");
	}
}
