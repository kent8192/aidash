use super::*;
use aidash_capability::InMemorySigner;
use axum::{
	Json, Router,
	extract::{Request, State},
	http::StatusCode,
	response::{IntoResponse, Response},
};
use std::sync::atomic::{AtomicUsize, Ordering};

const KID: &str =
	"projects/app/locations/us-central1/keyRings/broker/cryptoKeys/capability/cryptoKeyVersions/1";
const CANARY: &str = "canary-material-should-never-be-in-errors";
#[derive(Default)]
struct Tokens(AtomicUsize);
#[async_trait]
impl AccessTokenSource for Tokens {
	async fn token(&self) -> Result<SecretString, SignError> {
		self.0.fetch_add(1, Ordering::SeqCst);
		Ok("metadata-oauth".into())
	}
}
struct Api {
	mode: Mutex<String>,
	calls: AtomicUsize,
}
async fn handle(State(api): State<Arc<Api>>, request: Request) -> Response {
	api.calls.fetch_add(1, Ordering::SeqCst);
	let path = request.uri().path().to_owned();
	let mode = api.mode.lock().await.clone();
	if path.ends_with("/token") {
		assert_eq!(request.headers()["Metadata-Flavor"], "Google");
		return (
			[("Metadata-Flavor", "Google")],
			Json(
				serde_json::json!({"access_token":"metadata-oauth","token_type":"Bearer","expires_in":3600}),
			),
		)
			.into_response();
	}
	assert_eq!(request.headers()["Authorization"], "Bearer metadata-oauth");
	if mode == "denied" {
		return (StatusCode::FORBIDDEN, CANARY).into_response();
	}
	if mode == "disabled" {
		return (
			StatusCode::BAD_REQUEST,
			Json(
				serde_json::json!({"error":{"code":400,"status":"FAILED_PRECONDITION","message":CANARY}}),
			),
		)
			.into_response();
	}
	if mode == "outage" {
		return (StatusCode::SERVICE_UNAVAILABLE, CANARY).into_response();
	}
	if mode == "oversize" {
		return "x".repeat(65_537).into_response();
	}
	if path.ends_with(":asymmetricSign") {
		let body = axum::body::to_bytes(request.into_body(), 8192)
			.await
			.unwrap();
		let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
		assert!(value.get("digest").is_none());
		let input = STANDARD.decode(value["data"].as_str().unwrap()).unwrap();
		assert_eq!(value["dataCrc32c"], crc32c::crc32c(&input).to_string());
		let signature = InMemorySigner::new(KID.into(), [7; 32])
			.sign(&input)
			.await
			.unwrap();
		return Json(serde_json::json!({"name":if mode=="name" {"wrong"} else {KID},"signature":STANDARD.encode(signature),
			"signatureCrc32c":if mode=="checksum" {"0".into()} else {crc32c::crc32c(&signature).to_string()},"verifiedDataCrc32c":mode!="verified"})).into_response();
	}
	let name = path.trim_start_matches("/v1/").trim_end_matches(":access");
	Json(serde_json::json!({"name":if mode=="name" {"wrong"} else {name},"payload":{"data":STANDARD.encode(CANARY),
		"dataCrc32c":if mode=="checksum" {"0".into()} else {crc32c::crc32c(CANARY.as_bytes()).to_string()}}})).into_response()
}
struct Server {
	base: String,
	api: Arc<Api>,
	task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
	fn drop(&mut self) {
		self.task.abort();
	}
}
async fn server() -> Server {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let base = format!("http://{}/v1", listener.local_addr().unwrap());
	let api = Arc::new(Api {
		mode: Mutex::new("ok".into()),
		calls: AtomicUsize::new(0),
	});
	let router = Router::new().fallback(handle).with_state(api.clone());
	let task = tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	});
	Server { base, api, task }
}
#[tokio::test]
async fn kms_signs_raw_input_and_verifies_response_integrity_without_retry() {
	let s = server().await;
	let tokens = Arc::new(Tokens::default());
	let mut signer = KmsTokenSigner::new(KID.into(), tokens.clone()).unwrap();
	signer.base = s.base.clone();
	let signature = signer.sign(b"header.claims").await.unwrap();
	assert_eq!(
		signature,
		InMemorySigner::new(KID.into(), [7; 32])
			.sign(b"header.claims")
			.await
			.unwrap()
	);
	for mode in ["name", "checksum", "verified", "denied"] {
		*s.api.mode.lock().await = mode.into();
		assert_eq!(
			signer.sign(b"header.claims").await.unwrap_err().to_string(),
			"Capability Token signing unavailable"
		);
	}
	assert_eq!(s.api.calls.load(Ordering::SeqCst), 5);
	assert_eq!(tokens.0.load(Ordering::SeqCst), 5);
}
#[tokio::test]
async fn secret_manager_reads_only_pinned_environment_resources_and_redacts_errors() {
	let s = server().await;
	let mut source = SecretManagerKeyMaterialSource::new(
		"1234",
		"aidash-test-cred-",
		Arc::new(Tokens::default()),
	)
	.unwrap();
	source.base = s.base.clone();
	let secret = format!(
		"projects/1234/secrets/aidash-test-cred-{}",
		uuid::Uuid::from_u128(100)
	);
	assert_eq!(
		source.access(&secret, "1").await.unwrap().expose_secret(),
		CANARY
	);
	for mode in ["name", "checksum", "denied", "oversize"] {
		*s.api.mode.lock().await = mode.into();
		let error = source.access(&secret, "1").await.unwrap_err();
		assert!(!error.to_string().contains(CANARY));
	}
	let before = s.api.calls.load(Ordering::SeqCst);
	for (resource, version) in [
		(secret.replace("test", "other"), "1"),
		(secret.clone(), "latest"),
		(format!("{secret}/../../escape"), "1"),
	] {
		assert!(source.access(&resource, version).await.is_err());
	}
	assert_eq!(s.api.calls.load(Ordering::SeqCst), before);
}
#[tokio::test]
async fn disabled_secret_versions_are_permanent_while_store_outages_are_retryable() {
	let s = server().await;
	let mut source = SecretManagerKeyMaterialSource::new(
		"1234",
		"aidash-test-cred-",
		Arc::new(Tokens::default()),
	)
	.unwrap();
	source.base = s.base.clone();
	let secret = format!(
		"projects/1234/secrets/aidash-test-cred-{}",
		uuid::Uuid::from_u128(100)
	);
	for (mode, unavailable) in [("disabled", true), ("outage", false)] {
		*s.api.mode.lock().await = mode.into();
		let error = source.access(&secret, "1").await.unwrap_err();
		assert_eq!(matches!(error, KeyMaterialError::Unavailable), unavailable);
		assert!(!error.to_string().contains(CANARY));
	}
	assert_eq!(s.api.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn worker_audience_fits_the_bootstrap_service_account_name() {
	let mut configuration = issuer::WorkerConfiguration {
		endpoint: "https://broker.example/api/v1".into(),
		issuer: "worker".into(),
		audience: "a".repeat(16),
		kid: KID.into(),
	};
	configuration.validate().unwrap();
	configuration.audience.push('a');
	assert!(configuration.validate().is_err());
}

#[tokio::test]
async fn metadata_token_is_cached_and_requires_google_response_header() {
	let s = server().await;
	let mut tokens = MetadataTokenSource::new().unwrap();
	tokens.url = format!("{}/token", s.base);
	assert_eq!(
		tokens.token().await.unwrap().expose_secret(),
		"metadata-oauth"
	);
	tokens.token().await.unwrap();
	assert_eq!(s.api.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn production_constructors_reject_resource_traversal_and_latest_versions() {
	for kid in [
		"https://attacker.invalid",
		"projects/app/locations/us-central1/keyRings/broker/cryptoKeys/capability/cryptoKeyVersions/latest",
		"projects/../locations/us-central1/keyRings/broker/cryptoKeys/capability/cryptoKeyVersions/1",
	] {
		assert!(KmsTokenSigner::new(kid.into(), Arc::new(Tokens::default())).is_err());
	}
	assert!(
		SecretManagerKeyMaterialSource::new(
			"project-id",
			"aidash-test-cred-",
			Arc::new(Tokens::default())
		)
		.is_err()
	);
}
