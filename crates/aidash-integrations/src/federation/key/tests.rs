use super::*;
use aidash_application::federation::key::Signing;
use aidash_capability::TokenSigner;
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey, pkcs8::DecodePublicKey};
use rstest::rstest;

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
	use std::os::unix::fs::PermissionsExt;
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
	use std::os::unix::fs::PermissionsExt;
	std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn refused(path: &Path) -> String {
	FileDeploymentKey::load(path).unwrap_err().to_string()
}

/// A PKCS#8 key whose algorithm OID ends in `oid_arc`: 0x70 is Ed25519, 0x6e is
/// X25519, which has the same structure.
fn pem_with_algorithm(oid_arc: u8) -> String {
	let mut der = SigningKey::from_bytes(&[3; 32])
		.to_pkcs8_der()
		.unwrap()
		.as_bytes()
		.to_vec();
	let oid = der
		.windows(5)
		.position(|window| window == [0x06, 0x03, 0x2b, 0x65, 0x70])
		.unwrap();
	der[oid + 4] = oid_arc;
	let body = STANDARD.encode(der);
	let lines = body
		.as_bytes()
		.chunks(64)
		.map(|line| std::str::from_utf8(line).unwrap())
		.collect::<Vec<_>>()
		.join("\n");
	format!("-----BEGIN PRIVATE KEY-----\n{lines}\n-----END PRIVATE KEY-----\n")
}

#[test]
fn a_generated_key_loads_with_the_same_identity_and_is_private_to_its_owner() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("keys").join("deployment-key.pem");
	let generated = FileDeploymentKey::generate(&path, false).unwrap();
	let loaded = FileDeploymentKey::load(&path).unwrap();
	assert_eq!(loaded.key_id(), generated.key_id());
	assert_eq!(loaded.public_key_pem(), generated.public_key_pem());
	assert_eq!(loaded.key_id().as_str().len(), DeploymentKeyId::LEN);
	assert!(
		loaded
			.public_key_pem()
			.starts_with("-----BEGIN PUBLIC KEY-----")
	);
	#[cfg(unix)]
	assert_eq!(mode(&path), 0o600);
}

#[test]
fn the_key_id_digests_the_raw_public_key() {
	let key = FileDeploymentKey::new(SigningKey::from_bytes(&[9; 32])).unwrap();
	let public = VerifyingKey::from_public_key_pem(key.public_key_pem()).unwrap();
	assert_eq!(
		key.key_id(),
		&DeploymentKeyId::of_public_key(public.as_bytes())
	);
}

#[tokio::test]
async fn the_key_signs_the_exact_input_and_its_published_key_verifies_it() {
	let key = FileDeploymentKey::new(SigningKey::from_bytes(&[9; 32])).unwrap();
	let signature = key.sign(b"header.payload").await.unwrap();
	let public = VerifyingKey::from_public_key_pem(key.public_key_pem()).unwrap();
	public
		.verify_strict(b"header.payload", &Signature::from_bytes(&signature))
		.unwrap();
	assert!(
		public
			.verify_strict(b"header.other", &Signature::from_bytes(&signature))
			.is_err()
	);
	let signer = Signing(&key);
	assert_eq!(signer.kid(), key.key_id().as_str());
	assert_eq!(signer.sign(b"header.payload").await.unwrap(), signature);
}

#[test]
fn generate_keeps_an_existing_key_unless_it_is_told_to_replace_it() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("deployment-key.pem");
	let first = FileDeploymentKey::generate(&path, false).unwrap();
	let refusal = FileDeploymentKey::generate(&path, false).unwrap_err();
	assert!(matches!(refusal, Error::Conflict(_)), "{refusal}");
	assert_eq!(
		FileDeploymentKey::load(&path).unwrap().key_id(),
		first.key_id()
	);
	let second = FileDeploymentKey::generate(&path, true).unwrap();
	assert_ne!(second.key_id(), first.key_id());
	assert_eq!(
		FileDeploymentKey::load(&path).unwrap().key_id(),
		second.key_id()
	);
	assert!(!dir.path().join("deployment-key.pem.new").exists());
	#[cfg(unix)]
	assert_eq!(mode(&path), 0o600);
}

#[test]
fn a_missing_file_or_a_directory_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	assert!(refused(&dir.path().join("absent.pem")).contains("does not exist"));
	assert!(refused(dir.path()).contains("not a regular file"));
}

#[rstest]
#[case::empty("")]
#[case::text("not a key")]
#[case::certificate("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n")]
fn a_file_that_is_not_a_private_key_is_refused(#[case] content: &str) {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("deployment-key.pem");
	std::fs::write(&path, content).unwrap();
	#[cfg(unix)]
	set_mode(&path, 0o600);
	assert!(refused(&path).contains("not an Ed25519 private key"));
}

#[test]
fn another_key_algorithm_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("deployment-key.pem");
	std::fs::write(&path, pem_with_algorithm(0x70)).unwrap();
	#[cfg(unix)]
	set_mode(&path, 0o600);
	FileDeploymentKey::load(&path).unwrap();
	std::fs::write(&path, pem_with_algorithm(0x6e)).unwrap();
	assert!(refused(&path).contains("not an Ed25519 private key"));
}

#[cfg(unix)]
#[rstest]
#[case::group_readable(0o640)]
#[case::world_readable(0o644)]
#[case::group_writable(0o620)]
#[case::world_executable(0o601)]
fn a_key_that_others_can_reach_is_refused_without_echoing_it(#[case] bits: u32) {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("deployment-key.pem");
	let key = SigningKey::from_bytes(&[5; 32]);
	let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
	std::fs::write(&path, pem.as_bytes()).unwrap();
	set_mode(&path, bits);
	let message = refused(&path);
	assert!(
		message.contains("readable by its group or others"),
		"{message}"
	);
	assert!(!message.contains("PRIVATE KEY"), "{message}");
	set_mode(&path, 0o600);
	FileDeploymentKey::load(&path).unwrap();
	set_mode(&path, 0o400);
	FileDeploymentKey::load(&path).unwrap();
}

#[cfg(unix)]
#[test]
fn a_key_the_server_cannot_read_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("deployment-key.pem");
	FileDeploymentKey::generate(&path, false).unwrap();
	set_mode(&path, 0o000);
	// Superusers read through the mode; the check then has nothing to refuse.
	if File::open(&path).is_err() {
		assert!(refused(&path).contains("cannot be read"));
	}
}
