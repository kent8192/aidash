use crate::endpoint::{EndpointFuture, assert_json, endpoint};
use aidash_application::federation::key::DeploymentKey;
use aidash_integrations::federation::key::FileDeploymentKey;
use rstest::rstest;
use std::{collections::BTreeSet, sync::Arc};

fn fields(identity: &serde_json::Value) -> BTreeSet<&str> {
	identity
		.as_object()
		.unwrap()
		.keys()
		.map(String::as_str)
		.collect()
}

#[rstest]
#[tokio::test]
async fn a_deployment_without_a_key_publishes_its_identity_and_a_null_key(
	endpoint: EndpointFuture,
) {
	// Arrange
	let app = endpoint.await;
	// Act
	let identity = assert_json(app.anonymous.get("/.well-known/aidash").await.unwrap(), 200);
	// Assert
	assert_eq!(identity["id"], app.runtime.config.deployment);
	assert_eq!(identity["endpoint"], app.runtime.config.endpoint);
	assert_eq!(
		identity["protocol_version"],
		aidash_server::config::PROTOCOL_VERSION
	);
	assert!(identity["key"].is_null());
	assert_eq!(
		fields(&identity),
		BTreeSet::from(["endpoint", "id", "key", "protocol_version"]),
		"the document names no Node, capability or cluster"
	);
}

#[rstest]
#[tokio::test]
async fn a_deployment_with_a_key_file_publishes_the_key_id_and_the_public_key(
	endpoint: EndpointFuture,
) {
	// Arrange: the key the server's settings name, loaded the way startup loads it.
	let app = endpoint.await;
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join("deployment-key.pem");
	FileDeploymentKey::generate(&path, false).unwrap();
	let key = FileDeploymentKey::load(&path).unwrap();
	let (id, pem) = (key.key_id().to_string(), key.public_key_pem().to_owned());
	let mut runtime = app.runtime.clone();
	runtime.config.deployment_key = Some(Arc::new(key));
	app.context.set_singleton(runtime);
	// Act
	let identity = assert_json(app.anonymous.get("/.well-known/aidash").await.unwrap(), 200);
	// Assert
	assert_eq!(identity["key"]["id"], id.as_str());
	assert_eq!(identity["key"]["public_key_pem"], pem.as_str());
	assert_eq!(id.len(), 43);
	assert!(pem.starts_with("-----BEGIN PUBLIC KEY-----"));
	assert_eq!(
		fields(&identity["key"]),
		BTreeSet::from(["id", "public_key_pem"]),
		"the private key is never published"
	);
}

#[rstest]
#[tokio::test]
async fn scheduled_keys_are_shared_across_processes_and_published_without_source_references(
	endpoint: EndpointFuture,
) {
	use aidash_application::federation::key::{DeploymentKeySource, DeploymentKeys};
	let app = endpoint.await;
	let dir = tempfile::tempdir().unwrap();
	let old_path = dir.path().join("old.pem");
	let next_path = dir.path().join("successor.pem");
	let other_path = dir.path().join("other.pem");
	let old = FileDeploymentKey::generate(&old_path, false).unwrap();
	FileDeploymentKey::generate(&next_path, false).unwrap();
	FileDeploymentKey::generate(&other_path, false).unwrap();
	let settings = serde_json::from_value(serde_json::json!({ "file": old_path })).unwrap();
	let load = || {
		aidash_server::bootstrap::configure_deployment_keys(
			Some(&settings),
			dir.path(),
			app.runtime.config.deployment.clone(),
			app.runtime.store.pool.clone(),
		)
	};
	let first = load().await.unwrap().unwrap();
	let second = load().await.unwrap().unwrap();
	let now = chrono::Utc::now().timestamp() as u64 - 86_400;
	let (one, two) = tokio::join!(
		first.rollover(
			old.key_id(),
			DeploymentKeySource::File(next_path.clone()),
			now
		),
		second.rollover(old.key_id(), DeploymentKeySource::File(other_path), now)
	);
	assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
	let publication = first.publication(now).await.unwrap();
	let successor = publication.successor.unwrap();
	std::fs::remove_file(dir.path().join("old.pem")).unwrap();
	let connection = aidash_server::database::runtime::connect(&app.runtime.config.database_url, 2)
		.await
		.unwrap();
	let restarted = aidash_server::bootstrap::configure_deployment_keys(
		Some(&settings),
		dir.path(),
		app.runtime.config.deployment.clone(),
		connection.into_postgres().unwrap().into(),
	)
	.await
	.unwrap()
	.unwrap();
	assert_eq!(
		restarted
			.signing(now + 86_400)
			.await
			.unwrap()
			.key
			.key_id()
			.as_str(),
		successor.id
	);
	assert_eq!(
		second
			.signing(now + 86_400)
			.await
			.unwrap()
			.key
			.key_id()
			.as_str(),
		successor.id
	);
	let mut runtime = app.runtime.clone();
	runtime.store.egress.keys = Some(restarted);
	app.context.set_singleton(runtime);
	let identity = assert_json(app.anonymous.get("/.well-known/aidash").await.unwrap(), 200);
	assert_eq!(identity["key"]["id"], successor.id);
	assert_eq!(
		identity["key_rollover"]["retiring"]["id"],
		old.key_id().to_string()
	);
	assert!(identity["key_rollover"]["successor"].is_null());
	assert!(identity["key_rollover"]["transition"].is_string());
	let text = identity.to_string();
	assert!(!text.contains("PRIVATE KEY"));
	assert!(!text.contains(dir.path().to_str().unwrap()));
	assert!(!text.contains("source"));
}

#[path = "../../../execution/tests/support/deployment.rs"]
mod command_fixture;

/// Each command is a new OS process over the same authoritative schedule.
#[rstest]
#[tokio::test]
async fn operator_cli_generates_offline_and_persists_rollover_across_invocations(
	endpoint: EndpointFuture,
) {
	fn command(
		url: &str,
		directory: &std::path::Path,
		deployment: &str,
	) -> tokio::process::Command {
		let mut command = command_fixture::deployment_command(url, directory);
		let path = directory.join("settings/base.toml");
		let mut settings: toml::Value =
			toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
		settings["deployment"].as_table_mut().unwrap().insert(
			"key".into(),
			toml::Value::Table(toml::map::Map::from_iter([(
				"file".into(),
				toml::Value::String("old.pem".into()),
			)])),
		);
		std::fs::write(path, toml::to_string(&settings).unwrap()).unwrap();
		command.env("AIDASH_DEPLOYMENT_ID", deployment);
		command.arg("deployment-key");
		command
	}
	fn report(output: std::process::Output) -> String {
		assert!(
			output.status.success(),
			"{}",
			String::from_utf8_lossy(&output.stderr)
		);
		String::from_utf8(output.stdout).unwrap()
	}
	async fn output(command: &mut tokio::process::Command) -> std::process::Output {
		tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
			.await
			.expect("manage process did not produce output within 30 seconds")
			.unwrap()
	}
	let app = endpoint.await;
	let dir = tempfile::tempdir().unwrap();
	let deployment = app.runtime.config.deployment.to_string();
	let generated = report(
		output(command("postgres://127.0.0.1:1/offline", dir.path(), &deployment).arg("generate"))
			.await,
	);
	let id = generated
		.lines()
		.find_map(|line| line.strip_prefix("Key ID: "))
		.unwrap();
	let next_path = dir.path().join("next.pem");
	let next = FileDeploymentKey::generate(&next_path, false).unwrap();
	let url = &app.runtime.config.database_url;
	let absent = output(command(url, dir.path(), &deployment).args([
		"rollover",
		"--expected-current",
		id,
		"--file",
		"absent.pem",
	]))
	.await;
	assert!(!absent.status.success());
	let initial = report(output(command(url, dir.path(), &deployment).arg("status")).await);
	assert!(initial.contains(id));
	assert!(!initial.contains("Activates at:"));
	let published = report(
		output(command(url, dir.path(), &deployment).args([
			"rollover",
			"--expected-current",
			id,
			"--file",
			"next.pem",
		]))
		.await,
	);
	let restarted = report(output(command(url, dir.path(), &deployment).arg("status")).await);
	assert_eq!(published, restarted);
	assert!(restarted.contains(&format!("Successor key ID: {}", next.key_id())));
	assert!(restarted.contains("Activates at:"));
	assert!(!restarted.contains("PRIVATE KEY"));
	let conflicting = output(command(url, dir.path(), &deployment).args([
		"rollover",
		"--expected-current",
		id,
		"--file",
		"next.pem",
	]))
	.await;
	assert!(!conflicting.status.success());
	assert!(String::from_utf8_lossy(&conflicting.stderr).contains("already in progress"));
}
