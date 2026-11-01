use super::*;
use aidash_capability::{Failure, InMemorySigner, PublicKeys};
use secrecy::ExposeSecret;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

const KID: &str =
	"projects/app/locations/global/keyRings/server/cryptoKeys/secret/cryptoKeyVersions/1";

fn configuration() -> SecretIssuerConfiguration {
	SecretIssuerConfiguration {
		issuer: "server".into(),
		audience: "tenant-secret".into(),
		kid: KID.into(),
	}
}

fn request() -> SecretRequest {
	SecretRequest {
		tenant: "tenant-a".into(),
		secret: Uuid::from_u128(9),
		version: "3".into(),
		destination: Destination {
			origin: "https://api.example.com".into(),
			path_prefix: "/v1/".into(),
		},
		methods: vec!["GET".into(), "POST".into()],
		attach: Attach::Bearer,
		sub: SecretSubject::Run {
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

fn issuer(signer: &Arc<InMemorySigner>) -> SecretTokenIssuer {
	SecretTokenIssuer::new(configuration(), signer.clone()).unwrap()
}

fn issuers() -> BTreeMap<String, String> {
	BTreeMap::from([(KID.to_owned(), "server".to_owned())])
}

#[tokio::test]
async fn mints_a_tenant_secret_token_with_its_own_configuration_and_ttl() {
	// Arrange
	let signer = Arc::new(InMemorySigner::new(KID.into(), [7; 32]));
	let issuer = issuer(&signer);
	let request = request();
	// Act
	let token = issuer.mint(&request).await.unwrap();
	// Assert
	let mut keys = PublicKeys::default();
	keys.insert(KID.into(), signer.public_key());
	let claims = keys
		.verify_secret(token.expose_secret(), &issuers(), "tenant-secret", now())
		.unwrap();
	assert_eq!(claims.iss, "server");
	assert_eq!(claims.aud, "tenant-secret");
	assert_eq!(claims.kid, KID);
	assert_eq!(claims.exp - claims.iat, TOKEN_TTL_SECS);
	assert_eq!(claims.tenant, request.tenant);
	assert_eq!(claims.secret, request.secret);
	assert_eq!(claims.version, request.version);
	assert_eq!(claims.destination, request.destination);
	assert_eq!(claims.methods, request.methods);
	assert_eq!(claims.attach, request.attach);
	assert_eq!(claims.sub, request.sub);
	assert_eq!(claims.max_response_bytes, request.max_response_bytes);
	// The provider verifier never accepts the server's token kind.
	assert_eq!(
		keys.verify(token.expose_secret(), "server", "tenant-secret", now())
			.unwrap_err(),
		Failure::InvalidSignature
	);
}

#[test]
fn refuses_a_configuration_that_is_invalid_or_not_the_signers() {
	let signer = Arc::new(InMemorySigner::new(KID.into(), [7; 32]));
	let mut mismatched = configuration();
	mismatched.kid = "kms/worker".into();
	assert!(SecretTokenIssuer::new(mismatched, signer.clone()).is_err());
	for invalid in [
		SecretIssuerConfiguration {
			issuer: String::new(),
			..configuration()
		},
		SecretIssuerConfiguration {
			audience: String::new(),
			..configuration()
		},
		SecretIssuerConfiguration {
			audience: "Tenant-Secret".into(),
			..configuration()
		},
		SecretIssuerConfiguration {
			audience: "a".repeat(65),
			..configuration()
		},
		SecretIssuerConfiguration {
			kid: String::new(),
			..configuration()
		},
	] {
		assert!(SecretTokenIssuer::new(invalid, signer.clone()).is_err());
	}
}

#[tokio::test]
async fn refuses_to_mint_claims_the_broker_would_reject() {
	let signer = Arc::new(InMemorySigner::new(KID.into(), [7; 32]));
	let issuer = issuer(&signer);
	let mut lowercase_method = request();
	lowercase_method.methods = vec!["get".into()];
	assert!(issuer.mint(&lowercase_method).await.is_err());
	let mut plain_http = request();
	plain_http.destination.origin = "http://api.example.com".into();
	assert!(issuer.mint(&plain_http).await.is_err());
	let mut foreign_tenant = request();
	foreign_tenant.sub = SecretSubject::Operation {
		operation: Uuid::from_u128(11),
		tenant: "tenant-b".into(),
	};
	assert!(issuer.mint(&foreign_tenant).await.is_err());
	assert!(issuer.mint(&request()).await.is_ok());
}
