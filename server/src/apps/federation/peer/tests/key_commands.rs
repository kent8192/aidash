use super::{generate, show};
use crate::apps::operations::serializers::deployment::DeploymentKeySettings;
use crate::bootstrap::configure_deployment_key;
use crate::{Error, Result};
use std::path::Path;

fn file(name: &str) -> DeploymentKeySettings {
	serde_json::from_value(serde_json::json!({ "file": name })).unwrap()
}

fn kms() -> DeploymentKeySettings {
	serde_json::from_value(serde_json::json!({
		"kms": "projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1"
	}))
	.unwrap()
}

fn key_id(report: &str) -> &str {
	report
		.lines()
		.find_map(|line| line.strip_prefix("Key ID: "))
		.unwrap()
}

#[tokio::test]
async fn generate_then_show_report_the_same_key_and_startup_loads_it() {
	let dir = tempfile::tempdir().unwrap();
	let settings = file("keys/deployment-key.pem");
	let generated = generate(Some(&settings), dir.path(), false).unwrap();
	let shown = show(Some(&settings), dir.path()).await.unwrap();
	assert_eq!(key_id(&generated), key_id(&shown));
	assert_eq!(key_id(&shown).len(), 43);
	assert!(shown.contains("-----BEGIN PUBLIC KEY-----"));
	assert!(!generated.contains("PRIVATE KEY"));
	let loaded = configure_deployment_key(Some(&settings), dir.path())
		.await
		.unwrap()
		.unwrap();
	assert_eq!(loaded.key_id().as_str(), key_id(&shown));
}

#[test]
fn generate_keeps_an_existing_key_unless_forced() {
	let dir = tempfile::tempdir().unwrap();
	let settings = file("deployment-key.pem");
	let first = generate(Some(&settings), dir.path(), false).unwrap();
	let error = generate(Some(&settings), dir.path(), false).unwrap_err();
	assert!(matches!(error, Error::Conflict(_)), "{error}");
	assert!(error.to_string().contains("--force"));
	let second = generate(Some(&settings), dir.path(), true).unwrap();
	assert_ne!(key_id(&first), key_id(&second));
}

#[test]
fn generate_needs_a_file_in_the_settings() {
	let dir = tempfile::tempdir().unwrap();
	let none = generate(None, dir.path(), false).unwrap_err().to_string();
	assert!(none.contains("[deployment.key] file is not set"), "{none}");
	let kms = generate(Some(&kms()), dir.path(), false)
		.unwrap_err()
		.to_string();
	assert!(kms.contains("Cloud KMS"), "{kms}");
}

#[tokio::test]
async fn startup_has_no_key_without_settings_and_refuses_a_bad_one() {
	let dir = tempfile::tempdir().unwrap();
	assert!(
		configure_deployment_key(None, dir.path())
			.await
			.unwrap()
			.is_none()
	);
	let missing = configure_deployment_key(Some(&file("absent.pem")), dir.path()).await;
	assert!(
		missing
			.err()
			.unwrap()
			.to_string()
			.contains("does not exist")
	);
	std::fs::write(dir.path().join("bad.pem"), "not a key").unwrap();
	restrict(&dir.path().join("bad.pem"));
	let bad = configure_deployment_key(Some(&file("bad.pem")), dir.path()).await;
	assert!(
		bad.err()
			.unwrap()
			.to_string()
			.contains("not an Ed25519 private key")
	);
	let error = show(None, dir.path()).await.unwrap_err().to_string();
	assert!(error.contains("no [deployment.key]"), "{error}");
}

#[tokio::test]
async fn the_community_build_refuses_a_kms_key() {
	let error: Result<_> = configure_deployment_key(Some(&kms()), Path::new("/")).await;
	let error = error.err().unwrap().to_string();
	assert!(
		error.contains("the Community build has no Cloud KMS Deployment Key"),
		"{error}"
	);
}

#[cfg(unix)]
fn restrict(path: &Path) {
	use std::os::unix::fs::PermissionsExt;
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(not(unix))]
fn restrict(_: &Path) {}
