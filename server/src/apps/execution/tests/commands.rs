use reinhardt::test::fixtures::temp_dir;
use rstest::rstest;
use serde_json::Value;
use tempfile::TempDir;
use tokio::process::Command;

#[rstest]
#[tokio::test]
async fn schema_export_is_valid_json_without_runtime_settings_or_services(temp_dir: TempDir) {
	// Arrange: even an invalid runtime configuration must not block export.
	std::fs::write(temp_dir.path().join("base.toml"), "not valid TOML!").unwrap();
	// Act
	let mut command = Command::new(env!("CARGO_BIN_EXE_manage"));
	command
		.arg("exportopenapi")
		.env_clear()
		.env("PATH", std::env::var_os("PATH").unwrap_or_default())
		.env("REINHARDT_SETTINGS_DIR", temp_dir.path())
		.current_dir(temp_dir.path())
		.kill_on_drop(true);
	let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
		.await
		.expect("schema export must complete without waiting for services")
		.unwrap();
	// Assert
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	let schema: Value =
		serde_json::from_slice(&output.stdout).expect("stdout contains only the schema");
	assert_eq!(schema["info"]["title"], "Aidash API");
	assert!(schema["paths"]["/api/workspaces"]["post"].is_object());
	assert!(schema["paths"]["/api/events/stream"]["get"]["responses"]["200"]["content"]["text/event-stream"].is_object());
	assert!(schema["components"]["schemas"]["Policy"].is_object());
}

#[rstest]
#[case::help("--help", true, "Usage: manage runworker")]
#[case::invalid_address("not-an-address", false, "probe address must be")]
#[tokio::test]
async fn worker_command_validates_arguments_before_loading_runtime_settings(
	temp_dir: TempDir,
	#[case] argument: &str,
	#[case] success: bool,
	#[case] expected: &str,
) {
	// Arrange
	std::fs::write(temp_dir.path().join("base.toml"), "not valid TOML!").unwrap();
	let mut command = Command::new(env!("CARGO_BIN_EXE_manage"));
	command
		.args(["runworker", argument])
		.env_clear()
		.env("REINHARDT_SETTINGS_DIR", temp_dir.path())
		.current_dir(temp_dir.path())
		.kill_on_drop(true);
	// Act
	let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
		.await
		.expect("argument validation does not start background services")
		.unwrap();
	// Assert
	assert_eq!(output.status.success(), success);
	let text = if success {
		&output.stdout
	} else {
		&output.stderr
	};
	let text = String::from_utf8_lossy(text);
	assert!(text.contains(expected), "{text}");
}
