#[path = "support/environment.rs"]
mod environment;
use reinhardt::test::fixtures::temp_dir;
use rstest::rstest;
use serde_json::Value;
use tempfile::TempDir;
use tokio::process::Command;

#[rstest::fixture]
async fn management_process(
	#[from(reinhardt::test::fixtures::temp_dir)] temp_dir: tempfile::TempDir,
) -> tokio::sync::SemaphorePermit<'static> {
	// Newly linked macOS executables can spend tens of seconds in first-launch
	// validation. Warm both binaries once, then retain strict dispatch deadlines.
	if cfg!(target_os = "macos") {
		static WARMED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
		WARMED
			.get_or_init(|| async {
				let settings = temp_dir;
				std::fs::write(settings.path().join("base.toml"), "invalid TOML!").unwrap();
				for binary in [env!("CARGO_BIN_EXE_aidash"), env!("CARGO_BIN_EXE_manage")] {
					let mut command = Command::new(binary);
					command
						.arg("--help")
						.env_clear()
						.env("TOKIO_WORKER_THREADS", "2")
						.env("REINHARDT_SETTINGS_DIR", settings.path())
						.current_dir(settings.path())
						.kill_on_drop(true);
					let output =
						tokio::time::timeout(std::time::Duration::from_secs(90), command.output())
							.await
							.expect("macOS must finish first-launch validation within its deadline")
							.unwrap();
					assert!(output.status.success());
				}
			})
			.await;
	}
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
#[case::activation("activation-provision")]
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
#[case::toml("toml", "aidash://file-target")]
#[case::legacy("legacy", "aidash://legacy-target")]
#[case::composed("composed", "aidash://composed-target")]
#[case::dedicated("dedicated", "aidash://file-target")]
#[tokio::test]
async fn activation_provision_uses_composed_target_without_unrelated_runtime_settings(
	#[future(awt)]
	#[from(management_process)]
	_process_slot: tokio::sync::SemaphorePermit<'static>,
	temp_dir: TempDir,
	#[case] source: &str,
	#[case] node_id: &str,
	#[from(environment::nats_container)] nats_container: environment::NatsFuture,
) {
	use aidash_server::activation::{Broker, Settings};

	// Arrange: only node identity and NATS are available. Other fragments retain
	// unresolved secrets to prove this command requests a selected settings view.
	let (_container, url) = nats_container.await;

	let configured_url = if matches!(source, "legacy" | "composed" | "dedicated") {
		"nats://127.0.0.1:1"
	} else {
		&url
	};
	std::fs::write(
		temp_dir.path().join("base.toml"),
		format!(
			r#"
[node]
node_id = "aidash://file-target"
nats_url = "{configured_url}"
endpoint = "${{UNAVAILABLE_ENDPOINT}}"
api_token = "${{UNAVAILABLE_API_TOKEN}}"
[core]
secret_key = "${{UNAVAILABLE_CORE_SECRET}}"
"#
		),
	)
	.unwrap();
	let namespace = uuid::Uuid::new_v4().simple().to_string();
	let expected = Broker::names(
		node_id,
		&Settings {
			namespace: namespace.clone(),
			..Default::default()
		},
	);
	let nats = async_nats::connect(&url).await.unwrap();
	let jetstream = async_nats::jetstream::new(nats);
	for binary in [env!("CARGO_BIN_EXE_manage"), env!("CARGO_BIN_EXE_aidash")] {
		let mut command = Command::new(binary);
		command
			.arg("activation-provision")
			.env_clear()
			.env("TOKIO_WORKER_THREADS", "2")
			.env("REINHARDT_SETTINGS_DIR", temp_dir.path())
			.env("AIDASH_ACTIVATION_NAMESPACE", &namespace)
			.current_dir(temp_dir.path())
			.kill_on_drop(true);
		if source == "legacy" {
			command
				.env("AIDASH_NODE_ID", "aidash://legacy-target")
				.env("NATS_URL", &url);
		}
		if source == "composed" {
			std::fs::write(
				temp_dir.path().join("provision.toml"),
				format!(
					r#"[node]
node_id = "{node_id}"
nats_url = "${{REINHARDT_ACTIVATION_TEST_NATS}}"
"#
				),
			)
			.unwrap();
			command
				.env("REINHARDT_ENV", "provision")
				.env("REINHARDT_ACTIVATION_TEST_NATS", &url);
		}
		if source == "dedicated" {
			command.env("AIDASH_ACTIVATION_NATS_URL", &url);
		}
		// Act: both public binaries use the same registered Reinhardt command.
		let output = tokio::time::timeout(std::time::Duration::from_secs(15), command.output())
			.await
			.unwrap()
			.unwrap();
		// Assert: exact durable objects exist at the effective broker and node scope.
		assert!(
			output.status.success(),
			"{}",
			String::from_utf8_lossy(&output.stderr)
		);
		let result: Value = serde_json::from_slice(&output.stdout).unwrap();
		assert_eq!(result["stream"], expected.0);
		assert_eq!(result["subject"], expected.1);
		assert_eq!(result["consumer"], "workers-v1");
		assert!(result["consumer_created"].as_str().is_some());
		assert!(!String::from_utf8_lossy(&output.stdout).contains(&url));
		let mut stream = jetstream.get_stream(&expected.0).await.unwrap();
		assert_eq!(
			stream.info().await.unwrap().config.subjects,
			vec![expected.1.clone()]
		);
		assert!(
			stream
				.get_consumer::<async_nats::jetstream::consumer::pull::Config>("workers-v1")
				.await
				.is_ok()
		);
	}
	// The container owns all created streams and consumers, including failed cases.
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

#[rstest]
#[tokio::test]
async fn migration_seed_command_detects_drift_and_regenerates_only_owned_assets(
	#[future(awt)]
	#[from(management_process)]
	_process_slot: tokio::sync::SemaphorePermit<'static>,
	temp_dir: TempDir,
) {
	// Arrange: command selection must not deserialize unrelated invalid settings.
	std::fs::write(temp_dir.path().join("base.toml"), "invalid TOML!").unwrap();
	let root = temp_dir.path().join("migrations");
	let paths = ["federation", "marketplace"]
		.into_iter()
		.flat_map(|app| {
			["forward", "backward"]
				.into_iter()
				.map(move |direction| format!("{app}/sql/{direction}/0005_seed.sql"))
		})
		.collect::<Vec<_>>();
	for path in &paths {
		let path = root.join(path);
		std::fs::create_dir_all(path.parent().unwrap()).unwrap();
		std::fs::write(path, "seed drift").unwrap();
	}
	let untouched = root.join("other-history.rs");
	std::fs::write(&untouched, "unrelated history").unwrap();
	// Act: checking is read-only; regeneration is an explicit credential-free command.
	for (mode, success) in [("--check", false), ("--write", true), ("--check", true)] {
		let mut command = Command::new(env!("CARGO_BIN_EXE_manage"));
		command
			.args(["migrationseeds", mode, "--migration-dir"])
			.arg(&root)
			.env_clear()
			.env("TOKIO_WORKER_THREADS", "2")
			.env("REINHARDT_SETTINGS_DIR", temp_dir.path())
			.current_dir(temp_dir.path())
			.kill_on_drop(true);
		let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
			.await
			.unwrap()
			.unwrap();
		// Assert: only the requested generated assets can change.
		assert_eq!(
			output.status.success(),
			success,
			"{}",
			String::from_utf8_lossy(&output.stderr)
		);
		assert_eq!(
			std::fs::read_to_string(&untouched).unwrap(),
			"unrelated history"
		);
		if !success {
			assert!(
				String::from_utf8_lossy(&output.stderr)
					.contains("differs from its Reinhardt Query builder")
			);
			for path in &paths {
				assert_eq!(
					std::fs::read_to_string(root.join(path)).unwrap(),
					"seed drift"
				);
			}
		}
	}
}
