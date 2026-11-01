use super::*;
use aidash_capability::tenant_secret::{Attach, Destination, SecretSubject};
use aidash_capability::{InMemorySigner, PublicKeys};
use secrecy::ExposeSecret;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const KID: &str =
	"projects/app/locations/global/keyRings/server/cryptoKeys/secret/cryptoKeyVersions/1";

fn configuration() -> SecretIssuerConfiguration {
	SecretIssuerConfiguration {
		issuer: "aidash-server".into(),
		audience: "test-tsec".into(),
		kid: KID.into(),
	}
}

fn grant() -> SecretCallGrant {
	SecretCallGrant {
		tenant: "tenant-a".into(),
		secret: Uuid::from_u128(9),
		version: "3".into(),
		destination: Destination {
			origin: "https://api.example.com".into(),
			path_prefix: "/v1/".into(),
		},
		methods: vec!["GET".into(), "POST".into()],
		attach: Attach::Header {
			name: "x-api-key".into(),
		},
		subject: SecretSubject::Run {
			run: "run-1".into(),
			call: Uuid::from_u128(10),
		},
		max_response_bytes: Some(262_144),
	}
}

fn now() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_secs()
}

#[tokio::test]
async fn routes_to_the_forward_path_with_a_token_the_broker_verifies() {
	// Arrange
	let signer = Arc::new(InMemorySigner::new(KID.into(), [7; 32]));
	let calls =
		SecretCallsAdapter::with_signer("https://broker.run.app", configuration(), signer.clone())
			.unwrap();
	let grant = grant();
	// Act
	let route = calls.route(grant.clone()).await.unwrap();
	// Assert
	assert_eq!(route.base.as_str(), "https://broker.run.app/v1/forward");
	let mut keys = PublicKeys::default();
	keys.insert(KID.into(), signer.public_key());
	let issuers = BTreeMap::from([(KID.to_owned(), "aidash-server".to_owned())]);
	let claims = keys
		.verify_secret(route.token.expose_secret(), &issuers, "test-tsec", now())
		.unwrap();
	assert_eq!(claims.iss, "aidash-server");
	assert_eq!(claims.aud, "test-tsec");
	assert_eq!(claims.kid, KID);
	assert_eq!(claims.tenant, grant.tenant);
	assert_eq!(claims.secret, grant.secret);
	assert_eq!(claims.version, grant.version);
	assert_eq!(claims.destination, grant.destination);
	assert_eq!(claims.methods, grant.methods);
	assert_eq!(claims.attach, grant.attach);
	assert_eq!(claims.sub, grant.subject);
	assert_eq!(claims.max_response_bytes, grant.max_response_bytes);
}

#[test]
fn the_forward_base_is_the_bare_service_uri() {
	assert_eq!(
		forward_base("https://broker.run.app").unwrap().as_str(),
		"https://broker.run.app/v1/forward"
	);
	assert_eq!(
		forward_base("https://broker.run.app/").unwrap().as_str(),
		"https://broker.run.app/v1/forward"
	);
	assert_eq!(
		forward_base("http://127.0.0.1:8080").unwrap().as_str(),
		"http://127.0.0.1:8080/v1/forward"
	);
	for invalid in [
		"http://broker.run.app",
		"https://broker.run.app/api",
		"https://user@broker.run.app",
		"https://broker.run.app?x=1",
		"not a url",
	] {
		assert!(forward_base(invalid).is_err(), "{invalid}");
	}
}

#[tokio::test]
async fn a_configuration_that_does_not_match_the_signer_is_refused() {
	let signer = Arc::new(InMemorySigner::new("kms/worker".into(), [7; 32]));
	assert!(
		SecretCallsAdapter::with_signer("https://broker.run.app", configuration(), signer).is_err()
	);
}
