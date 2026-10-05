use reinhardt::test::fixtures::temp_dir;
use rstest::rstest;
use serde_json::Value;
use tempfile::TempDir;
use tokio::process::Command;

#[rstest::fixture]
async fn management_process() -> tokio::sync::SemaphorePermit<'static> {
	// Bound concurrent cold starts of the same large, freshly linked binary.
	// The permit survives through output collection and releases on timeout.
	static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
	SLOTS.acquire().await.unwrap()
}

#[rstest]
#[tokio::test]
async fn schema_export_is_valid_json_without_runtime_settings_or_services(
	#[future(awt)]
	#[from(management_process)]
	_process_slot: tokio::sync::SemaphorePermit<'static>,
	temp_dir: TempDir,
) {
	// Arrange: even an invalid runtime configuration must not block export.
	std::fs::write(temp_dir.path().join("base.toml"), "not valid TOML!").unwrap();
	// Act
	let mut command = Command::new(env!("CARGO_BIN_EXE_manage"));
	command
		.arg("exportopenapi")
		.env_clear()
		.env("TOKIO_WORKER_THREADS", "2")
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
	#[future(awt)]
	#[from(management_process)]
	_process_slot: tokio::sync::SemaphorePermit<'static>,

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
		.env("TOKIO_WORKER_THREADS", "2")
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

#[rstest]
#[case::serve("serve")]
#[case::server("server")]
#[case::worker("worker")]
#[case::openapi("openapi")]
#[case::diagnostics("diagnostics")]
#[tokio::test]
async fn deployment_aliases_use_reinhardt_commands_before_runtime_configuration(
	#[future(awt)]
	#[from(management_process)]
	_process_slot: tokio::sync::SemaphorePermit<'static>,

	temp_dir: TempDir,
	#[case] name: &str,
) {
	// Arrange
	std::fs::write(temp_dir.path().join("base.toml"), "invalid TOML!").unwrap();
	// Act: both binaries select the same registered command without opening DB/HTTP.
	for binary in [env!("CARGO_BIN_EXE_aidash"), env!("CARGO_BIN_EXE_manage")] {
		let mut command = Command::new(binary);
		command
			.args([name, "--help"])
			.env_clear()
			.env("TOKIO_WORKER_THREADS", "2")
			.env("REINHARDT_SETTINGS_DIR", temp_dir.path())
			.current_dir(temp_dir.path())
			.kill_on_drop(true);
		let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
			.await
			.unwrap()
			.unwrap();
		// Assert
		assert!(
			output.status.success(),
			"{}",
			String::from_utf8_lossy(&output.stderr)
		);
		let text = String::from_utf8_lossy(&output.stdout);
		assert!(text.contains(name), "{text}");
		assert!(text.contains("Usage:"), "{text}");
	}
}

#[rstest]
#[case::acceptance("acceptance")]
#[case::memory("remote-memory")]
#[tokio::test]
async fn diagnostics_are_registered_static_commands_instead_of_example_binaries(
	#[future(awt)]
	#[from(management_process)]
	_process_slot: tokio::sync::SemaphorePermit<'static>,

	temp_dir: TempDir,
	#[case] profile: &str,
) {
	// Arrange
	std::fs::write(temp_dir.path().join("base.toml"), "invalid TOML!").unwrap();
	let mut command = Command::new(env!("CARGO_BIN_EXE_manage"));
	command
		.args(["diagnostics", profile])
		.env_clear()
		.env("TOKIO_WORKER_THREADS", "2")
		.env("REINHARDT_SETTINGS_DIR", temp_dir.path())
		.current_dir(temp_dir.path())
		.kill_on_drop(true);
	// Act
	let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
		.await
		.unwrap()
		.unwrap();
	// Assert
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	let document: Value = serde_json::from_slice(&output.stdout).unwrap();
	assert!(!document.as_object().unwrap().is_empty());
	assert!(document.as_object().unwrap().values().any(|value| {
		value
			.as_str()
			.is_some_and(|query| query.contains("SELECT") || query.contains("UPDATE"))
	}));
}
