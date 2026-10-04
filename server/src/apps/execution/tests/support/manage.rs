//! Run the actual manage binary against an isolated native database fixture.
use crate::deployment::deployment_command;
use crate::native_database::{DatabaseFixture, database};
use reinhardt::test::APIClient;
use reinhardt::test::fixtures::{api_client_from_url, temp_dir};
use rstest::fixture;
use std::{fs::File, net::TcpListener, process::Stdio, time::Duration};
use tempfile::TempDir;
use tokio::process::{Child, Command};

pub struct ManageFixture {
	pub client: APIClient,
	pub probes: APIClient,
	pub process: Child,
	pub database: DatabaseFixture,
	pub directory: TempDir,
}

#[fixture]
pub async fn native_server(
	#[default("container")] profile: &str,
	#[future] database: DatabaseFixture,
	temp_dir: TempDir,
) -> ManageFixture {
	start(database.await, temp_dir, false, profile).await
}

#[fixture]
pub async fn native_worker(
	#[future] database: DatabaseFixture,
	temp_dir: TempDir,
) -> ManageFixture {
	start(database.await, temp_dir, true, "container").await
}

async fn start(
	database: DatabaseFixture,
	temp_dir: TempDir,
	worker: bool,
	profile: &str,
) -> ManageFixture {
	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	let address = listener.local_addr().unwrap();
	let probe_listener = TcpListener::bind("127.0.0.1:0").unwrap();
	let probe_address = if worker {
		address
	} else {
		probe_listener.local_addr().unwrap()
	};
	let mut command = deployment_command(&database.url, temp_dir.path());
	command.env("REINHARDT_ENV", profile);
	if profile == "oidc" {
		command.envs([
			(
				"AIDASH_OIDC_ISSUER",
				"https://identity.example/realms/fixture",
			),
			("AIDASH_OIDC_CLIENT_ID", "fixture-dashboard"),
			("AIDASH_OIDC_CLIENT_SECRET", "fixture-dashboard-secret"),
			("AIDASH_OIDC_PUBLIC_ORIGIN", "https://dashboard.example"),
			(
				"AIDASH_OIDC_KEYCLOAK_ADMIN_URL",
				"https://identity.example/admin/realms/fixture",
			),
			("AIDASH_OIDC_STATUS_CLIENT_ID", "fixture-status"),
			("AIDASH_OIDC_STATUS_CLIENT_SECRET", "fixture-status-secret"),
		]);
	}
	let log = File::create(temp_dir.path().join("manage.log")).unwrap();
	drop(listener);
	drop(probe_listener);
	if worker {
		command.args(["runworker", &address.to_string()]);
	} else {
		command.args(["runserver", &address.to_string(), "--noreload"]);
	}
	let process = command
		.env("AIDASH_ENDPOINT", format!("http://{address}"))
		.env("AIDASH_PROBE_LISTEN", probe_address.to_string())
		.stdin(Stdio::null())
		.stdout(log.try_clone().unwrap())
		.stderr(log)
		.kill_on_drop(true)
		.spawn()
		.expect("start native manage process");
	let client = api_client_from_url(&format!("http://{address}"));
	let probes = api_client_from_url(&format!("http://{probe_address}"));
	let mut fixture = ManageFixture {
		client,
		probes,
		process,
		database,
		directory: temp_dir,
	};
	let probe = reqwest::Client::builder()
		.timeout(Duration::from_millis(500))
		.build()
		.unwrap();
	let ready = tokio::time::timeout(Duration::from_secs(30), async {
		loop {
			if let Some(status) = fixture.process.try_wait().unwrap() {
				panic!("manage exited {status}: {}", fixture.log());
			}
			let runtime_ready = probe
				.get(format!("http://{probe_address}/ready"))
				.send()
				.await
				.is_ok_and(|response| response.status().is_success());
			// The startup hook exposes probes before runserver binds its HTTP
			// listener. Wait for both listeners before returning a server fixture.
			let http_ready = worker
				|| probe
					.get(format!("http://{address}/.well-known/aidash"))
					.send()
					.await
					.is_ok_and(|response| response.status().is_success());
			if runtime_ready && http_ready {
				break;
			}
			tokio::time::sleep(Duration::from_millis(50)).await;
		}
	})
	.await;
	assert!(
		ready.is_ok(),
		"native manage process did not become ready: {}",
		fixture.log()
	);
	fixture
		.client
		.set_header("Authorization", "Bearer native-command-test-operator")
		.await
		.unwrap();
	fixture
}

impl ManageFixture {
	pub fn log(&self) -> String {
		std::fs::read_to_string(self.directory.path().join("manage.log")).unwrap()
	}

	pub async fn shutdown(&mut self) {
		#[cfg(unix)]
		{
			let status = Command::new("kill")
				.args([
					"-TERM",
					&self.process.id().expect("running process").to_string(),
				])
				.status()
				.await
				.unwrap();
			assert!(status.success());
			let status = tokio::time::timeout(Duration::from_secs(25), self.process.wait())
				.await
				.expect("native graceful shutdown deadline")
				.unwrap();
			assert!(
				status.success(),
				"manage failed during shutdown: {}",
				self.log()
			);
		}
		#[cfg(not(unix))]
		self.process.kill().await.unwrap();
	}
}
