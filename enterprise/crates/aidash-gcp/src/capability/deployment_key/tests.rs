use super::*;
use aidash_capability::InMemorySigner;
use axum::{
	Json, Router,
	extract::{Request, State},
	http::StatusCode,
	response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::pkcs8::EncodePublicKey;

const VERSION: &str = "projects/app/locations/us-central1/keyRings/federation/cryptoKeys/deployment/cryptoKeyVersions/1";

struct Tokens;
#[async_trait]
impl AccessTokenSource for Tokens {
	async fn token(&self) -> Result<secrecy::SecretString, SignError> {
		Ok("metadata-oauth".into())
	}
}

fn key_material() -> InMemorySigner {
	InMemorySigner::new(VERSION.into(), [7; 32])
}

fn public_pem() -> String {
	let der = key_material().public_key().to_public_key_der().unwrap();
	let body = STANDARD.encode(der.as_bytes());
	format!("-----BEGIN PUBLIC KEY-----\n{body}\n-----END PUBLIC KEY-----\n")
}

struct Api {
	mode: &'static str,
	available: std::sync::atomic::AtomicBool,
}

async fn handle(State(api): State<Arc<Api>>, request: Request) -> Response {
	assert_eq!(request.headers()["Authorization"], "Bearer metadata-oauth");
	let mode = api.mode;
	let path = request.uri().path().to_owned();
	if mode == "outage" || !api.available.load(std::sync::atomic::Ordering::SeqCst) {
		return (StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
	}
	let version = if path.contains("/cryptoKeyVersions/2") {
		VERSION.trim_end_matches('1').to_owned() + "2"
	} else {
		VERSION.into()
	};
	let material = InMemorySigner::new(
		version.clone(),
		[if version.ends_with("/2") { 8 } else { 7 }; 32],
	);
	if path.ends_with(":asymmetricSign") {
		if mode == "sign_denied" {
			return (StatusCode::FORBIDDEN, "sign denied").into_response();
		}
		let body = axum::body::to_bytes(request.into_body(), 8192)
			.await
			.unwrap();
		let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
		let input = STANDARD.decode(value["data"].as_str().unwrap()).unwrap();
		let signature = material.sign(&input).await.unwrap();
		return Json(serde_json::json!({
			"name": version, "signature": STANDARD.encode(signature),
			"signatureCrc32c": crc32c::crc32c(&signature).to_string(), "verifiedDataCrc32c": true
		}))
		.into_response();
	}
	assert!(path.ends_with(&format!("{version}/publicKey")), "{path}");
	let pem = if mode == "garbage" {
		"not a pem".to_owned()
	} else {
		material.public_key_pem()
	};
	Json(serde_json::json!({
		"name": if mode == "name" { "other" } else { &version },
		"algorithm": if mode == "algorithm" { "EC_SIGN_P256_SHA256" } else { ED25519 },
		"pem": pem,
		"pemCrc32c": if mode == "checksum" { "0".into() } else { crc32c::crc32c(pem.as_bytes()).to_string() },
		"protectionLevel": "HSM"
	}))
	.into_response()
}

struct Server {
	base: String,
	task: tokio::task::JoinHandle<()>,
	api: Arc<Api>,
}
impl Drop for Server {
	fn drop(&mut self) {
		self.task.abort();
	}
}
async fn server(mode: &'static str) -> Server {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let base = format!("http://{}/v1", listener.local_addr().unwrap());
	let api = Arc::new(Api {
		mode,
		available: std::sync::atomic::AtomicBool::new(true),
	});
	let router = Router::new().fallback(handle).with_state(api.clone());
	let task = tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	});
	Server { base, task, api }
}

async fn fetch(server: &Server) -> Result<KmsDeploymentKey, SignError> {
	let mut signer = KmsTokenSigner::new(VERSION.into(), Arc::new(Tokens)).unwrap();
	signer.base = server.base.clone();
	KmsDeploymentKey::fetch(signer).await
}

#[tokio::test]
async fn the_published_key_is_the_kms_key_and_its_signatures_verify_against_it() {
	let s = server("ok").await;
	let key = fetch(&s).await.unwrap();
	let expected = key_material().public_key();
	assert_eq!(
		key.key_id(),
		&DeploymentKeyId::of_public_key(expected.as_bytes())
	);
	assert_eq!(key.key_id().as_str().len(), DeploymentKeyId::LEN);
	assert_eq!(key.public_key_pem(), public_pem());
	let signature = key.sign(b"header.claims").await.unwrap();
	expected
		.verify_strict(
			b"header.claims",
			&ed25519_dalek::Signature::from_bytes(&signature),
		)
		.unwrap();
}

#[tokio::test]
async fn a_damaged_or_foreign_public_key_is_refused() {
	for mode in ["name", "algorithm", "checksum", "garbage", "outage"] {
		let s = server(mode).await;
		assert!(fetch(&s).await.is_err(), "{mode}");
	}
}

#[test]
fn only_a_full_key_version_resource_name_is_accepted() {
	for version in [
		"",
		"projects/p/locations/l/keyRings/r/cryptoKeys/k",
		"projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/latest",
		"projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/0",
		"projects/../locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1",
	] {
		let built = tokio::runtime::Builder::new_current_thread()
			.build()
			.unwrap()
			.block_on(KmsDeploymentKey::new(version.into(), Arc::new(Tokens)));
		assert!(built.is_err(), "{version}");
	}
}

/// KMS source selection uses the real HTTP signer against the existing fake
/// KMS API. The fake owns no schedule or activation logic.
#[tokio::test]
async fn scheduled_kms_versions_activate_and_fail_closed_during_an_outage() {
	use aidash_application::federation::key::{
		DeploymentKeyLoader, DeploymentKeyRollover, DeploymentKeySource, DeploymentKeyStore,
		DeploymentKeys, ScheduledDeploymentKeys,
	};
	use aidash_domain::nodes::DeploymentId;
	struct Sources(String);
	#[async_trait]
	impl DeploymentKeyLoader for Sources {
		async fn load(
			&self,
			source: &DeploymentKeySource,
		) -> aidash_application::Result<Arc<dyn DeploymentKey>> {
			let DeploymentKeySource::Kms(version) = source else {
				panic!("KMS fixture");
			};
			let mut signer = KmsTokenSigner::new(version.clone(), Arc::new(Tokens)).unwrap();
			signer.base = self.0.clone();
			Ok(Arc::new(KmsDeploymentKey::fetch(signer).await.map_err(
				|_| aidash_application::Error::External("KMS unavailable".into()),
			)?))
		}
	}
	#[derive(Default)]
	struct Schedule(std::sync::Mutex<Option<DeploymentKeyRollover>>);
	#[async_trait]
	impl DeploymentKeyStore for Schedule {
		async fn load(&self) -> aidash_application::Result<Option<DeploymentKeyRollover>> {
			Ok(self.0.lock().unwrap().clone())
		}
		async fn compare_and_set(
			&self,
			expected: Option<&DeploymentKeyRollover>,
			next: &DeploymentKeyRollover,
		) -> aidash_application::Result<bool> {
			let mut state = self.0.lock().unwrap();
			if state.as_ref() != expected {
				return Ok(false);
			}
			*state = Some(next.clone());
			Ok(true)
		}
	}
	let api_server = server("ok").await;
	let state = Arc::new(Schedule::default());
	let ring = || {
		ScheduledDeploymentKeys::new(
			DeploymentId::parse("aidash://sender").unwrap(),
			DeploymentKeySource::Kms(VERSION.into()),
			state.clone(),
			Arc::new(Sources(api_server.base.clone())),
		)
	};
	let old = DeploymentKeyId::of_public_key(key_material().public_key().as_bytes());
	let version = VERSION.trim_end_matches('1').to_owned() + "2";
	let new = DeploymentKeyId::of_public_key(
		InMemorySigner::new("unused".into(), [8; 32])
			.public_key()
			.as_bytes(),
	);
	let now = 1_800_000_000;
	ring()
		.rollover(&old, DeploymentKeySource::Kms(version), now)
		.await
		.unwrap();
	assert_eq!(ring().signing(now).await.unwrap().key.key_id(), &old);
	let active = ring().signing(now + 86_400).await.unwrap();
	assert_eq!(active.key.key_id(), &new);
	assert!(active.transition.is_some());
	let signature = active.key.sign(b"request").await.unwrap();
	InMemorySigner::new("unused".into(), [8; 32])
		.public_key()
		.verify_strict(
			b"request",
			&ed25519_dalek::Signature::from_bytes(&signature),
		)
		.unwrap();
	api_server
		.api
		.available
		.store(false, std::sync::atomic::Ordering::SeqCst);
	assert!(active.key.sign(b"request").await.is_err());
	assert!(ring().signing(now + 86_400).await.is_err());
	assert_eq!(
		ring().publication(now + 86_400).await.unwrap().current.id,
		new.to_string()
	);
	let denied = server("sign_denied").await;
	let unpublished = Arc::new(Schedule::default());
	let denied_ring = ScheduledDeploymentKeys::new(
		DeploymentId::parse("aidash://sender").unwrap(),
		DeploymentKeySource::Kms(VERSION.into()),
		unpublished.clone(),
		Arc::new(Sources(denied.base.clone())),
	);
	assert!(
		denied_ring
			.rollover(
				&old,
				DeploymentKeySource::Kms(VERSION.trim_end_matches('1').to_owned() + "2"),
				now
			)
			.await
			.is_err()
	);
	assert!(
		unpublished.load().await.unwrap().is_none(),
		"readable public KMS metadata does not prove signing permission"
	);
}
