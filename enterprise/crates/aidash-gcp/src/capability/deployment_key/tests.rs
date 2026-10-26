// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

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
}

async fn handle(State(api): State<Arc<Api>>, request: Request) -> Response {
	assert_eq!(request.headers()["Authorization"], "Bearer metadata-oauth");
	let mode = api.mode;
	let path = request.uri().path().to_owned();
	if mode == "outage" {
		return (StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
	}
	if path.ends_with(":asymmetricSign") {
		let body = axum::body::to_bytes(request.into_body(), 8192)
			.await
			.unwrap();
		let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
		let input = STANDARD.decode(value["data"].as_str().unwrap()).unwrap();
		let signature = key_material().sign(&input).await.unwrap();
		return Json(serde_json::json!({
			"name": VERSION, "signature": STANDARD.encode(signature),
			"signatureCrc32c": crc32c::crc32c(&signature).to_string(), "verifiedDataCrc32c": true
		}))
		.into_response();
	}
	assert!(path.ends_with(&format!("{VERSION}/publicKey")), "{path}");
	let pem = if mode == "garbage" {
		"not a pem".to_owned()
	} else {
		public_pem()
	};
	Json(serde_json::json!({
		"name": if mode == "name" { "other" } else { VERSION },
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
}
impl Drop for Server {
	fn drop(&mut self) {
		self.task.abort();
	}
}
async fn server(mode: &'static str) -> Server {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let base = format!("http://{}/v1", listener.local_addr().unwrap());
	let api = Arc::new(Api { mode });
	let router = Router::new().fallback(handle).with_state(api);
	let task = tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	});
	Server { base, task }
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
