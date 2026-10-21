//! A disposable native worker using the same isolated database as its HTTP fixture.
use aidash_server::federation::Federation;
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use reinhardt::test::fixtures::temp_dir;
use rstest::fixture;
use std::{
	fs::File,
	process::{Child, Command, Stdio},
};
use tempfile::TempDir;

use crate::common::{process_settings, settings_for};

pub type ConfigurationFuture =
	Shared<BoxFuture<'static, (Federation, String, String, Option<std::path::PathBuf>)>>;

#[fixture]
pub fn worker_configuration(
	#[from(crate::common::runtime)] runtime: crate::common::RuntimeFuture,
) -> ConfigurationFuture {
	async move {
		let (runtime, url, schema) = runtime.await.parts();
		(runtime, url, schema, None)
	}
	.boxed()
	.shared()
}

pub struct WorkerCommand {
	command: Command,
	directory: TempDir,
}

#[fixture]
pub fn worker_command(
	worker_configuration: ConfigurationFuture,
	temp_dir: TempDir,
) -> BoxFuture<'static, WorkerCommand> {
	async move {
		let (runtime, url, schema, memory) = worker_configuration.await;
		let mut settings = settings_for(&url);
		settings.deployment.id = runtime.config.deployment.to_string();
		settings.deployment.endpoint = runtime.config.endpoint.clone();
		settings.deployment.api_token = runtime.config.api_token.clone();
		settings.deployment.nats_url = runtime.config.nats_url.clone();
		settings.deployment.lease_seconds = runtime.config.lease_seconds;
		settings.deployment.background_enabled = true;
		settings.deployment.worker_count = 1;
		// The process signs with the Deployment's own key.
		crate::common::peering::configure_process(&mut settings, &runtime);
		// A worker process must reach the same Keyless fixtures (ADR 0036).
		settings.keyless_endpoints.permitted_private_origins =
			runtime.store.keyless.permitted_origins();
		// A runtime with a Provider Credential Store gives its worker a self-hosted
		// Store, so the worker admits definitions that use Tenant credentials.
		let provider_store = runtime.store.provider_credentials.is_some();
		if provider_store {
			use aidash_server::apps::identity::serializers::provider_credentials::{
				KeySource, StoreConfig,
			};
			let key = |env: &str| KeySource {
				file: None,
				env: Some(env.into()),
			};
			settings.provider_credentials.fingerprint_key =
				Some(key("AIDASH_TEST_PROVIDER_FINGERPRINT_KEY"));
			settings.provider_credentials.store = Some(StoreConfig::Postgres {
				master_key: key("AIDASH_TEST_PROVIDER_STORE_MASTER_KEY"),
				retired_master_keys: vec![],
			});
		}
		settings
			.core
			.databases
			.get_mut("default")
			.unwrap()
			.options
			.insert("options".into(), format!("-c application_name={schema}"));
		std::fs::write(
			temp_dir.path().join("base.toml"),
			process_settings(&settings),
		)
		.unwrap();
		let log = File::create(temp_dir.path().join("worker.log")).unwrap();
		let mut command = Command::new(env!("CARGO_BIN_EXE_aidash"));
		command
			// The database fixture already applied migrations; exercise worker recovery directly.
			.args(["runworker"])
			.env_clear()
			.env("PATH", std::env::var_os("PATH").unwrap_or_default())
			.env("REINHARDT_SETTINGS_DIR", temp_dir.path())
			.env("REINHARDT_ENV", "test")
			.env("RUST_BACKTRACE", "0")
			.current_dir(env!("CARGO_MANIFEST_DIR"))
			.stdin(Stdio::null())
			.stdout(log.try_clone().unwrap())
			.stderr(log);
		if let Some(memory) = memory {
			command.env("AIDASH_MEMORY_RECOVERY_DIR", memory);
		}
		if provider_store {
			command
				.env(
					"AIDASH_TEST_PROVIDER_FINGERPRINT_KEY",
					"worker-process-fingerprint-key-of-32-bytes",
				)
				.env("AIDASH_TEST_PROVIDER_STORE_MASTER_KEY", "1f".repeat(32));
		}
		WorkerCommand {
			command,
			directory: temp_dir,
		}
	}
	.boxed()
}

#[fixture]
pub async fn worker_process(
	#[from(worker_configuration)] _configuration: ConfigurationFuture,
	#[future(awt)]
	#[with(_configuration.clone())]
	worker_command: WorkerCommand,
) -> WorkerProcess {
	WorkerProcess::spawn(worker_command)
}

pub struct WorkerProcess {
	pub process: Child,
	_directory: TempDir,
}

impl WorkerProcess {
	pub fn spawn(mut prepared: WorkerCommand) -> Self {
		Self {
			process: prepared.command.spawn().unwrap(),
			_directory: prepared.directory,
		}
	}
	#[allow(dead_code)] // Process suites use this when startup or execution stalls.
	pub fn log(&self) -> String {
		std::fs::read_to_string(self._directory.path().join("worker.log")).unwrap()
	}

	#[allow(dead_code)] // This shared fixture is compiled by suites with different startup paths.
	pub fn start(runtime: &Federation, url: &str, schema: &str, directory: TempDir) -> Self {
		Self::start_configured(runtime, url, schema, None, directory)
	}
	#[allow(dead_code)] // Native-memory suites additionally require the independent Home ledger.
	pub fn start_with_memory(
		runtime: &Federation,
		url: &str,
		schema: &str,
		memory: &std::path::Path,
		directory: TempDir,
	) -> Self {
		Self::start_configured(runtime, url, schema, Some(memory), directory)
	}
	fn start_configured(
		runtime: &Federation,
		url: &str,
		schema: &str,
		memory: Option<&std::path::Path>,
		directory: TempDir,
	) -> Self {
		let mut settings = settings_for(url);
		settings.deployment.id = runtime.config.deployment.to_string();
		settings.deployment.endpoint = runtime.config.endpoint.clone();
		settings.deployment.api_token = runtime.config.api_token.clone();
		settings.deployment.nats_url = runtime.config.nats_url.clone();
		settings.deployment.lease_seconds = runtime.config.lease_seconds;
		settings.deployment.background_enabled = true;
		settings.deployment.worker_count = 1;
		// The process signs with the Deployment's own key.
		crate::common::peering::configure_process(&mut settings, runtime);
		// A worker process must reach the same Keyless fixtures (ADR 0036).
		settings.keyless_endpoints.permitted_private_origins =
			runtime.store.keyless.permitted_origins();
		settings
			.core
			.databases
			.get_mut("default")
			.unwrap()
			.options
			.insert("options".into(), format!("-c application_name={schema}"));
		std::fs::write(
			directory.path().join("base.toml"),
			process_settings(&settings),
		)
		.unwrap();
		let log = File::create(directory.path().join("worker.log")).unwrap();
		let mut command = Command::new(env!("CARGO_BIN_EXE_aidash"));
		command
			// Fixtures have already applied the native migration graph. Exercise
			// the worker lifecycle directly; deployment migration is tested by
			// the command suites and must not consume the provider-cut deadline.
			.args(["runworker"])
			.env_clear()
			.env("PATH", std::env::var_os("PATH").unwrap_or_default())
			.env("REINHARDT_SETTINGS_DIR", directory.path())
			.env("REINHARDT_ENV", "test")
			.env("RUST_BACKTRACE", "0")
			.current_dir(env!("CARGO_MANIFEST_DIR"))
			.stdin(Stdio::null())
			.stdout(log.try_clone().unwrap())
			.stderr(log);
		if let Some(memory) = memory {
			command.env("AIDASH_MEMORY_RECOVERY_DIR", memory);
		}
		let process = command.spawn().unwrap();
		Self {
			process,
			_directory: directory,
		}
	}
}

impl Drop for WorkerProcess {
	fn drop(&mut self) {
		let _ = self.process.kill();
		let _ = self.process.wait();
	}
}
