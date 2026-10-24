//! A signed GCIP login must yield a usable, bounded lifecycle proof. The key
//! lookup and upstream enabled fact are fixture inputs; TokenVerifier still
//! verifies RS256, issuer, audience, tenant and authentication time itself.
use super::*;
use aidash_application::ports::authorization::dashboard::AccountStatus as UpstreamStatus;
use aidash_domain::identity::dashboard::AccountState;
use aidash_integrations::jwks::SigningKeys;
use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header};

struct FixtureKeys;
#[async_trait]
impl SigningKeys for FixtureKeys {
	async fn key(&self, kid: &str) -> aidash_integrations::Result<DecodingKey> {
		if kid != "fixture-key" {
			return Err(aidash_integrations::Error::Unauthorized);
		}
		DecodingKey::from_rsa_pem(include_bytes!(
			"../../../../../../crates/aidash-gcp/src/gcip/signing-test-only-public.pem"
		))
		.map_err(|_| aidash_integrations::Error::Unauthorized)
	}
}
struct EnabledSource;
#[async_trait]
impl UpstreamStatus for EnabledSource {
	async fn lookup(&self, _: &str, _: &str) -> aidash_application::Result<AccountState> {
		Ok(AccountState::default())
	}
}

struct ChangedSource {
	disabled: bool,
}
#[async_trait]
impl UpstreamStatus for ChangedSource {
	async fn lookup(&self, _: &str, _: &str) -> aidash_application::Result<AccountState> {
		Ok(AccountState {
			disabled: self.disabled,
			valid_since: Some(Utc::now() - chrono::Duration::seconds(1)),
		})
	}
}

async fn begin_login(fixture: &Fixture, browser: &str) -> (String, Value) {
	let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
	let mut authorize = url::Url::parse(&format!("{}/oauth/authorize", fixture.issuer)).unwrap();
	authorize.query_pairs_mut().extend_pairs([
		("client_id", "marketplace"),
		("redirect_uri", fixture.callback.as_str()),
		("response_type", "code"),
		("scope", "openid"),
		("code_challenge", challenge.as_str()),
		("code_challenge_method", "S256"),
		("state", "signup-state"),
		("nonce", "signup-nonce"),
	]);
	let response = fixture
		.client
		.get(authorize)
		.header("cookie", format!("aidash-auth={browser}"))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 302);
	let flow = url::Url::parse(&format!(
		"{}{}",
		fixture.issuer,
		response.headers()["location"].to_str().unwrap()
	))
	.unwrap()
	.query_pairs()
	.find(|(key, _)| key == "flow")
	.unwrap()
	.1
	.into_owned();
	let context = fixture
		.client
		.get(format!("{}/login/context?flow={flow}", fixture.issuer))
		.header("cookie", format!("aidash-auth={browser}"))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	(flow, context)
}

fn signed_token(subject: &str, auth_time: i64, extra: Value) -> String {
	let now = Utc::now().timestamp();
	let mut header = Header::new(Algorithm::RS256);
	header.kid = Some("fixture-key".into());
	let mut claims = json!({
		"iss":"https://securetoken.google.com/fixture-project",
		"aud":"fixture-project", "sub":subject, "iat":now, "exp":now+3600,
		"auth_time":auth_time,
		"firebase":{"tenant":"shared","sign_in_provider":"password"},
		"email_verified":true, "email":"person@example.test"
	});
	claims
		.as_object_mut()
		.unwrap()
		.extend(extra.as_object().unwrap().clone());
	jsonwebtoken::encode(
		&header,
		&claims,
		&EncodingKey::from_rsa_pem(include_bytes!(
			"../../../../../../crates/aidash-gcp/src/gcip/signing-test-only.pem"
		))
		.unwrap(),
	)
	.unwrap()
}
#[tokio::test]
async fn browser_signup_consumes_proof_anchored_to_verified_upstream_authentication() {
	let fixture = fixture(|auth| {
		auth.verifier = Arc::new(aidash_gcp::gcip::TokenVerifier {
			project: "fixture-project".into(),
			keys: Arc::new(FixtureKeys),
		});
		auth.upstream = Arc::new(EnabledSource);
	})
	.await;
	let browser = secret();
	let (flow, context) = begin_login(&fixture, &browser).await;
	let subject = Uuid::new_v4().to_string();
	let token = signed_token(
		&subject,
		Utc::now().timestamp() - 2,
		json!({"account_id":fixture.account.id,"is_admin":true,"name":"Source A"}),
	);
	let mut parts: Vec<String> = token.split('.').map(str::to_owned).collect();
	let mut signature = URL_SAFE_NO_PAD.decode(&parts[2]).unwrap();
	signature[0] ^= 1;
	parts[2] = URL_SAFE_NO_PAD.encode(signature);
	let rejected = fixture
		.client
		.post(format!("{}/login/verify", fixture.issuer))
		.header("origin", &fixture.issuer)
		.header("cookie", format!("aidash-auth={browser}"))
		.json(&json!({"flow":flow,"csrf":context["csrf"],"id_token":parts.join(".")}))
		.send()
		.await
		.unwrap();
	assert_eq!(
		rejected.status(),
		401,
		"browser input cannot bypass the upstream signature verifier"
	);
	let response = fixture
		.client
		.post(format!("{}/login/verify", fixture.issuer))
		.header("origin", &fixture.issuer)
		.header("cookie", format!("aidash-auth={browser}"))
		.json(&json!({"flow":flow,"csrf":context["csrf"],"id_token":token}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
	let result: Value = response.json().await.unwrap();
	assert_eq!(result["phase"], "choose-account");
	let response = fixture
		.client
		.post(format!("{}/login/choice", fixture.issuer))
		.header("origin", &fixture.issuer)
		.header("cookie", format!("aidash-auth={browser}"))
		.json(&json!({"flow":flow,"csrf":context["csrf"],"choice":"new"}))
		.send()
		.await
		.unwrap();
	assert_eq!(
		response.status(),
		200,
		"fresh verified login must create an Account: {}",
		response.text().await.unwrap()
	);
	let result: Value = response.json().await.unwrap();
	assert!(
		result["redirect_to"]
			.as_str()
			.unwrap()
			.starts_with(&fixture.callback)
	);
	let mut scope = fixture.auth.store.scope().await.unwrap();
	let source = scope
		.identity_by_source(
			"https://securetoken.google.com/fixture-project/tenants/shared",
			&subject,
		)
		.await
		.unwrap()
		.unwrap();
	assert!(source.linked);
	assert_ne!(
		source.first_account,
		Some(fixture.account.id),
		"upstream custom claims cannot choose a common Account"
	);
	drop(scope);
	let callback = url::Url::parse(result["redirect_to"].as_str().unwrap()).unwrap();
	let code = callback
		.query_pairs()
		.find(|(key, _)| key == "code")
		.unwrap()
		.1
		.into_owned();
	let tokens = exchange(&fixture.client, &fixture.issuer, &fixture.callback, &code).await;
	let claims = source_claims(&fixture, &tokens).await;
	assert_eq!(claims["source_identity_id"], source.id.to_string());
	assert_eq!(
		claims["email"], "person@example.test",
		"verified source email is required by Cloud demo fulfilment"
	);
	assert_eq!(claims["email_verified"], true);
	assert_eq!(claims["name"], "Source A");
	assert!(claims.get("is_admin").is_none());
	assert_ne!(claims["account_id"], fixture.account.id.to_string());
	let sso_code = authorize(
		&fixture.client,
		&fixture.issuer,
		&fixture.callback,
		&browser,
		&secret(),
	)
	.await;
	let sso_tokens = exchange(
		&fixture.client,
		&fixture.issuer,
		&fixture.callback,
		&sso_code,
	)
	.await;
	let sso_claims = source_claims(&fixture, &sso_tokens).await;
	assert_eq!(sso_claims["email"], "person@example.test");
	assert_eq!(sso_claims["email_verified"], true);
	assert_eq!(sso_claims["name"], "Source A");
	assert_eq!(sso_claims["source_identity_id"], source.id.to_string());
	let inspected = introspect(
		&fixture.client,
		&fixture.issuer,
		sso_tokens["access_token"].as_str().unwrap(),
	)
	.await;
	assert_eq!(inspected["email"], "person@example.test");
	assert_eq!(inspected["name"], "Source A");

	// Proving A may link B to its Account, but cannot lend A's verified email
	// or display name to B. B's own Google token contains an unverified email.
	let other_browser = secret();
	let other_subject = Uuid::new_v4().to_string();
	let (other_flow, other_context) = begin_login(&fixture, &other_browser).await;
	let other_token = signed_token(
		&other_subject,
		Utc::now().timestamp() - 2,
		json!({"email":"unverified@example.test","email_verified":false,"name":"Source B",
			"firebase":{"tenant":"shared","sign_in_provider":"google.com"}}),
	);
	let other_result = verify_fixture_login(
		&fixture,
		&other_browser,
		&other_flow,
		&other_context,
		&other_token,
	)
	.await;
	assert_eq!(other_result["phase"], "choose-account");
	let response = fixture
		.client
		.post(format!("{}/login/choice", fixture.issuer))
		.header("origin", &fixture.issuer)
		.header("cookie", format!("aidash-auth={other_browser}"))
		.json(&json!({"flow":other_flow,"csrf":other_context["csrf"],"choice":"existing"}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	let proving: Value = response.json().await.unwrap();
	assert_eq!(proving["phase"], "prove-existing");
	let current_token = signed_token(
		&subject,
		Utc::now().timestamp() - 1,
		json!({"name":"Current A"}),
	);
	let linked = verify_fixture_login(
		&fixture,
		&other_browser,
		&other_flow,
		&proving,
		&current_token,
	)
	.await;
	let callback = url::Url::parse(linked["redirect_to"].as_str().unwrap()).unwrap();
	let code = callback
		.query_pairs()
		.find(|(key, _)| key == "code")
		.unwrap()
		.1
		.into_owned();
	let other_tokens = exchange(&fixture.client, &fixture.issuer, &fixture.callback, &code).await;
	let other_claims = source_claims(&fixture, &other_tokens).await;
	assert_eq!(other_claims["account_id"], claims["account_id"]);
	assert_eq!(other_claims["sub"], claims["sub"]);
	assert_eq!(other_claims["source_subject"], other_subject);
	assert_ne!(
		other_claims["source_identity_id"],
		claims["source_identity_id"]
	);
	assert!(
		other_claims.get("email").is_none(),
		"a verified email from A cannot cross the source boundary"
	);
	assert!(other_claims.get("email_verified").is_none());
	assert_eq!(other_claims["name"], "Source B");
	let other_sso_code = authorize(
		&fixture.client,
		&fixture.issuer,
		&fixture.callback,
		&other_browser,
		&secret(),
	)
	.await;
	let other_sso_tokens = exchange(
		&fixture.client,
		&fixture.issuer,
		&fixture.callback,
		&other_sso_code,
	)
	.await;
	let other_sso_claims = source_claims(&fixture, &other_sso_tokens).await;
	assert_eq!(
		other_sso_claims["source_identity_id"],
		other_claims["source_identity_id"]
	);
	assert_eq!(other_sso_claims["name"], "Source B");
	assert!(other_sso_claims.get("email").is_none());
	let original = source_claims(&fixture, &tokens).await;
	assert_eq!(original["email"], "person@example.test");
	assert_eq!(
		original["name"], "Source A",
		"linking cannot replace an existing session's verified snapshot"
	);
}

async fn verify_fixture_login(
	fixture: &Fixture,
	browser: &str,
	flow: &str,
	context: &Value,
	token: &str,
) -> Value {
	let response = fixture
		.client
		.post(format!("{}/login/verify", fixture.issuer))
		.header("origin", &fixture.issuer)
		.header("cookie", format!("aidash-auth={browser}"))
		.json(&json!({"flow":flow,"csrf":context["csrf"],"id_token":token}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
	response.json().await.unwrap()
}

async fn source_claims(fixture: &Fixture, tokens: &Value) -> Value {
	let response = fixture
		.client
		.get(format!("{}/oauth/userinfo", fixture.issuer))
		.bearer_auth(tokens["access_token"].as_str().unwrap())
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	response.json().await.unwrap()
}

#[rstest::rstest]
#[case::disabled(true)]
#[case::session_cutoff(false)]
#[tokio::test]
async fn rejected_browser_login_persists_definite_upstream_stop(#[case] disabled: bool) {
	let fixture = fixture(|auth| {
		auth.verifier = Arc::new(aidash_gcp::gcip::TokenVerifier {
			project: "fixture-project".into(),
			keys: Arc::new(FixtureKeys),
		});
		auth.upstream = Arc::new(ChangedSource { disabled });
	})
	.await;
	let cutoff = Utc::now() - chrono::Duration::seconds(1);
	let previous = fixture
		.auth
		.issue_session(
			fixture.identity.id,
			"cloud",
			cutoff - chrono::Duration::seconds(10),
		)
		.await
		.unwrap();
	let browser = secret();
	let (flow, context) = begin_login(&fixture, &browser).await;
	let token = signed_token(&fixture.identity.subject, cutoff.timestamp() - 1, json!({}));
	let response = fixture
		.client
		.post(format!("{}/login/verify", fixture.issuer))
		.header("origin", &fixture.issuer)
		.header("cookie", format!("aidash-auth={browser}"))
		.json(&json!({"flow":flow,"csrf":context["csrf"],"id_token":token}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 401);
	let mut scope = fixture.auth.store.scope().await.unwrap();
	let source = scope.identity(fixture.identity.id).await.unwrap().unwrap();
	assert_eq!(
		source.status,
		if disabled {
			IdentityStatus::Disabled
		} else {
			IdentityStatus::Enabled
		}
	);
	assert_eq!(source.disable_epoch, u64::from(disabled));
	assert!(
		scope
			.session(previous.id)
			.await
			.unwrap()
			.unwrap()
			.revoked_at
			.is_some(),
		"a definite upstream observation must invalidate old interactive sessions immediately"
	);
	drop(scope);
	let (validity, _, _) = fixture
		.auth
		.check_origin(
			fixture.identity.id,
			Some(fixture.account.id),
			Some(0),
			Some(0),
			false,
		)
		.await
		.unwrap();
	assert_eq!(
		validity,
		if disabled {
			aidash_domain::accounts::Validity::Denied(
				aidash_domain::accounts::Denial::IdentityDisabled,
			)
		} else {
			aidash_domain::accounts::Validity::Active
		},
		"a session cutoff alone must not stop durable work"
	);
}
