use super::*;
use aidash_application::ports::authorization::dashboard::AccountStatus;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, encode};
use rstest::rstest;
use serde_json::{Value, json};

struct Keys;
#[async_trait]
impl SigningKeys for Keys {
	async fn key(&self, kid: &str) -> Result<DecodingKey> {
		if kid != "fixture" {
			return Err(Error::Unauthorized);
		}
		Ok(DecodingKey::from_rsa_pem(include_bytes!("signing-test-only-public.pem")).unwrap())
	}
}
fn token(claims: &Value) -> String {
	let mut header = Header::new(Algorithm::RS256);
	header.kid = Some("fixture".into());
	encode(
		&header,
		claims,
		&EncodingKey::from_rsa_pem(include_bytes!("signing-test-only.pem")).unwrap(),
	)
	.unwrap()
}
fn claims() -> Value {
	let now = Utc::now().timestamp();
	json!({"iss":"https://securetoken.google.com/fixture-project", "aud":"fixture-project", "sub":"person", "iat":now, "exp":now+3600, "auth_time":now, "firebase":{"tenant":"pool-a", "sign_in_provider":"password"}, "email_verified":true, "email":"person@example.test", "name":"Person", "deployment_administrator":true})
}
#[rstest]
#[case::issuer("iss", json!("https://issuer.example.test"))]
#[case::audience("aud", json!("other-project"))]
#[case::expired("exp", json!(1))]
#[case::old_authentication("auth_time", json!(1))]
#[case::future_authentication("auth_time", json!(9_000_000_000_i64))]
#[case::unverified_password("email_verified", json!(false))]
#[case::missing_authentication("auth_time", Value::Null)]
#[case::missing_provider("firebase", json!({"tenant":"pool-a"}))]
#[case::empty_uid("sub", json!(""))]
#[tokio::test]
async fn signed_tokens_with_invalid_claims_are_rejected(#[case] field: &str, #[case] value: Value) {
	let mut claims = claims();
	claims[field] = value;
	let verifier = TokenVerifier {
		project: "fixture-project".into(),
		keys: Arc::new(Keys),
	};
	assert!(matches!(
		verifier.verify(&token(&claims), "pool-a", Utc::now()).await,
		Err(Error::Unauthorized)
	));
}
#[tokio::test]
async fn signed_token_is_scoped_to_transaction_tenant_and_ignores_authority_claims() {
	let verifier = TokenVerifier {
		project: "fixture-project".into(),
		keys: Arc::new(Keys),
	};
	let token = token(&claims());
	let (sign_in, method) = verifier.verify(&token, "pool-a", Utc::now()).await.unwrap();
	assert_eq!(
		sign_in.issuer,
		"https://securetoken.google.com/fixture-project/tenants/pool-a"
	);
	assert_eq!(method, "password");
	assert_eq!(sign_in.subject, "person");
	assert_eq!(
		sign_in.verified_email.as_deref(),
		Some("person@example.test")
	);
	assert!(matches!(
		verifier.verify(&token, "unbound-pool", Utc::now()).await,
		Err(Error::Unauthorized)
	));
}
// GCIP enforces the MFA Requirement; the claims it adds after a second factor
// are never authority (ADR 0003), so they must not change what a sign-in is.
#[rstest]
#[case::absent(None)]
#[case::totp(Some(("totp", "enrollment-id")))]
#[case::phone(Some(("phone", "enrollment-id")))]
#[tokio::test]
async fn second_factor_claims_never_change_the_verified_sign_in(
	#[case] second_factor: Option<(&str, &str)>,
) {
	let verifier = TokenVerifier {
		project: "fixture-project".into(),
		keys: Arc::new(Keys),
	};
	let mut claims = claims();
	if let Some((factor, identifier)) = second_factor {
		claims["firebase"]["sign_in_second_factor"] = json!(factor);
		claims["firebase"]["second_factor_identifier"] = json!(identifier);
	}
	let (sign_in, method) = verifier
		.verify(&token(&claims), "pool-a", Utc::now())
		.await
		.unwrap();
	assert_eq!(sign_in.subject, "person");
	assert_eq!(
		sign_in.issuer,
		"https://securetoken.google.com/fixture-project/tenants/pool-a"
	);
	assert_eq!(method, "password");
	assert_eq!(
		sign_in.verified_email.as_deref(),
		Some("person@example.test")
	);
	assert_eq!(sign_in.display_name.as_deref(), Some("Person"));
	assert_eq!(sign_in.auth_time.timestamp(), claims["auth_time"]);
}
#[tokio::test]
async fn tampered_signature_and_unsigned_tokens_are_rejected() {
	let verifier = TokenVerifier {
		project: "fixture-project".into(),
		keys: Arc::new(Keys),
	};
	let token = token(&claims());
	let (prefix, signature) = token.rsplit_once('.').unwrap();
	let first = if signature.starts_with('A') { 'B' } else { 'A' };
	let tampered = format!("{prefix}.{first}{}", &signature[1..]);
	assert!(matches!(
		verifier.verify(&tampered, "pool-a", Utc::now()).await,
		Err(Error::Unauthorized)
	));
	assert!(matches!(
		verifier
			.verify("eyJhbGciOiJub25lIn0.e30.", "pool-a", Utc::now())
			.await,
		Err(Error::Unauthorized)
	));
}
#[tokio::test]
async fn federated_unverified_email_is_never_saved_as_verified() {
	let verifier = TokenVerifier {
		project: "fixture-project".into(),
		keys: Arc::new(Keys),
	};
	let mut claims = claims();
	claims["firebase"]["sign_in_provider"] = json!("saml.enterprise");
	claims["email_verified"] = json!(false);
	assert!(
		verifier
			.verify(&token(&claims), "pool-a", Utc::now())
			.await
			.unwrap()
			.0
			.verified_email
			.is_none()
	);
}
struct Credentials;
#[async_trait]
impl AccessToken for Credentials {
	async fn token(&self) -> Result<String> {
		Ok("fixture-access".into())
	}
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
#[rstest]
#[case::enabled(json!({"users":[{"localId":"person", "disabled":false,"validSince":"1800000000","tenantId":"pool-a"}]}), false, Some(1_800_000_000))]
#[case::disabled(json!({"users":[{"localId":"person", "disabled":true}]}), true, None)]
#[case::deleted(json!({}), true, None)]
#[tokio::test]
async fn admin_lookup_is_tenant_scoped_and_returns_both_status_values(
	#[case] body: Value,
	#[case] disabled: bool,
	#[case] valid_since: Option<i64>,
) {
	use axum::{Json, Router, http::HeaderMap, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let router = Router::new().route(
		"/v1/projects/fixture-project/tenants/pool-a/accounts:lookup",
		post(move |headers: HeaderMap, Json(request): Json<Value>| {
			let body = body.clone();
			async move {
				assert_eq!(headers["authorization"], "Bearer fixture-access");
				assert_eq!(request, json!({"localId":["person"]}));
				Json(body)
			}
		}),
	);
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap()
	}));
	let lookup = AccountLookup {
		client: reqwest::Client::new(),
		project: "fixture-project".into(),
		endpoint,
		credentials: Arc::new(Credentials),
	};
	let pool = |tenant: &str| super::issuer("fixture-project", tenant);
	let status = lookup.lookup(&pool("pool-a"), "person").await.unwrap();
	assert_eq!(status.disabled, disabled);
	assert_eq!(status.valid_since.map(|time| time.timestamp()), valid_since);
	for other in [
		"https://securetoken.google.com/fixture-project".to_owned(),
		pool("other-pool"),
		super::issuer("other-project", "pool-a"),
	] {
		assert!(lookup.lookup(&other, "person").await.is_err(), "{other}");
	}
}

#[rstest]
#[case::wrong_subject(json!({"users":[{"localId":"someone-else"}]}))]
#[case::wrong_pool(json!({"users":[{"localId":"person","tenantId":"other"}]}))]
#[case::invalid_timestamp(json!({"users":[{"localId":"person","validSince":"not-a-time"}]}))]
#[tokio::test]
async fn admin_lookup_fails_closed_for_malformed_or_foreign_users(#[case] body: Value) {
	use axum::{Json, Router, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let router = Router::new().route(
		"/v1/projects/fixture-project/tenants/pool-a/accounts:lookup",
		post(move || {
			let body = body.clone();
			async { Json(body) }
		}),
	);
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap()
	}));
	let lookup = AccountLookup {
		client: reqwest::Client::new(),
		project: "fixture-project".into(),
		endpoint,
		credentials: Arc::new(Credentials),
	};
	assert!(
		lookup
			.lookup(&super::issuer("fixture-project", "pool-a"), "person")
			.await
			.is_err()
	);
}
