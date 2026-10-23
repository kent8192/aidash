use super::*;
use axum::{
	Form, Json, Router,
	http::{HeaderMap, StatusCode},
	routing::{get, post},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}

#[derive(Default)]
struct Seen {
	token_forms: Vec<BTreeMap<String, String>>,
	revocations: Vec<BTreeMap<String, String>>,
	bearers: Vec<String>,
}

/// A fake OpenAI whose token endpoint answers with `token`.
async fn fake(
	token: (StatusCode, Value),
	revoke: StatusCode,
	models: (StatusCode, Value),
) -> (ChatgptAuthorization, Arc<Mutex<Seen>>, Server) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	let seen = Arc::new(Mutex::new(Seen::default()));
	let mut jwks: Value =
		serde_json::from_str(include_str!("../jwks/signing-test-only.jwks.json")).unwrap();
	jwks["keys"][0]["kid"] = json!("fixture");
	let discovery = json!({
		"issuer": ISSUER,
		"token_endpoint": format!("{url}/oauth/token"),
		"jwks_uri": format!("{url}/jwks"),
		"revocation_endpoint": format!("{url}/oauth/revoke"),
	});
	let (tokens, revocations, bearers) = (seen.clone(), seen.clone(), seen.clone());
	let app = Router::new()
		.route(
			"/.well-known/openid-configuration",
			get(move || async move { Json(discovery) }),
		)
		.route("/jwks", get(move || async move { Json(jwks) }))
		.route(
			"/oauth/token",
			post(move |Form(form): Form<BTreeMap<String, String>>| {
				let seen = tokens.clone();
				let token = token.clone();
				async move {
					seen.lock().unwrap().token_forms.push(form);
					(token.0, Json(token.1))
				}
			}),
		)
		.route(
			"/oauth/revoke",
			post(move |Form(form): Form<BTreeMap<String, String>>| {
				let seen = revocations.clone();
				async move {
					seen.lock().unwrap().revocations.push(form);
					revoke
				}
			}),
		)
		.route(
			"/v1/models",
			get(move |headers: HeaderMap| {
				let seen = bearers.clone();
				let models = models.clone();
				async move {
					seen.lock()
						.unwrap()
						.bearers
						.push(headers["authorization"].to_str().unwrap().to_owned());
					(models.0, Json(models.1))
				}
			}),
		);
	let server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	(
		ChatgptAuthorization::at(
			reqwest::Client::new(),
			format!("{url}/.well-known/openid-configuration"),
			format!("{url}/v1"),
		),
		seen,
		server,
	)
}
fn tokens(scope: &str) -> Value {
	json!({
		"access_token":"access-canary","refresh_token":"refresh-canary","id_token":"id",
		"token_type":"Bearer","expires_in":3600,"scope":scope
	})
}
fn id_token(claims: Value, kid: &str) -> SecretString {
	let mut header = Header::new(Algorithm::RS256);
	header.kid = Some(kid.into());
	encode(
		&header,
		&claims,
		&EncodingKey::from_rsa_pem(include_bytes!("../jwks/signing-test-only.pem")).unwrap(),
	)
	.unwrap()
	.into()
}
fn claims(client: &str, nonce: &str) -> Value {
	let now = chrono::Utc::now().timestamp();
	json!({"iss":ISSUER,"aud":client,"sub":"user-1","exp":now + 300,"iat":now,"nonce":nonce})
}
fn ok_models() -> (StatusCode, Value) {
	(StatusCode::OK, json!({"models":[]}))
}

#[tokio::test]
async fn exchanges_send_the_verifier_redirect_and_resource() {
	let (authorization, seen, _server) = fake(
		(StatusCode::OK, tokens(domain::SCOPES)),
		StatusCode::OK,
		ok_models(),
	)
	.await;
	let verifier = SecretString::from("verifier-1".to_owned());
	let issued = authorization
		.exchange(ExchangeRequest {
			client_id: "oaiapp_1",
			code: "code-1",
			verifier: &verifier,
			redirect_uri: "http://127.0.0.1:43123/auth/callback",
		})
		.await
		.unwrap();
	assert_eq!(issued.access_token.expose_secret(), "access-canary");
	assert_eq!(issued.refresh_token.expose_secret(), "refresh-canary");
	assert_eq!(issued.expires_in, 3600);
	assert_eq!(issued.scope.as_deref(), Some(domain::SCOPES));
	let form = &seen.lock().unwrap().token_forms[0];
	assert_eq!(form["grant_type"], "authorization_code");
	assert_eq!(form["client_id"], "oaiapp_1");
	assert_eq!(form["code"], "code-1");
	assert_eq!(form["code_verifier"], "verifier-1");
	assert_eq!(form["redirect_uri"], "http://127.0.0.1:43123/auth/callback");
	assert_eq!(form["resource"], "https://api.openai.com/v1");
	assert!(!form.contains_key("client_secret"));
}

#[tokio::test]
async fn rejected_codes_fail_without_echoing_the_provider_body() {
	for (status, invalid) in [
		(StatusCode::BAD_REQUEST, true),
		(StatusCode::UNAUTHORIZED, true),
		(StatusCode::INTERNAL_SERVER_ERROR, false),
	] {
		let (authorization, _, _server) = fake(
			(
				status,
				json!({"error":"invalid_grant","error_description":"canary refresh-leak"}),
			),
			StatusCode::OK,
			ok_models(),
		)
		.await;
		let verifier = SecretString::from("v".to_owned());
		let error = authorization
			.exchange(ExchangeRequest {
				client_id: "oaiapp_1",
				code: "code",
				verifier: &verifier,
				redirect_uri: "http://127.0.0.1:1457/auth/callback",
			})
			.await
			.err()
			.unwrap();
		assert_eq!(matches!(error, Error::Invalid(_)), invalid, "{status}");
		assert!(!format!("{error:?}").contains("canary"));
	}
}

#[tokio::test]
async fn id_tokens_must_name_this_client_and_nonce() {
	let (authorization, _, _server) = fake(
		(StatusCode::OK, tokens(domain::SCOPES)),
		StatusCode::OK,
		ok_models(),
	)
	.await;
	let valid = id_token(claims("oaiapp_1", "nonce-1"), "fixture");
	// The verified subject identifies the ChatGPT account.
	assert_eq!(
		authorization
			.verify_id_token(&valid, "oaiapp_1", "nonce-1")
			.await
			.unwrap(),
		"user-1"
	);
	for (token, client, nonce) in [
		(valid.clone(), "oaiapp_2", "nonce-1"),
		(valid.clone(), "oaiapp_1", "nonce-2"),
		(
			id_token(
				json!({"iss":"https://attacker.test","aud":"oaiapp_1","sub":"u","exp":chrono::Utc::now().timestamp() + 300,"nonce":"nonce-1"}),
				"fixture",
			),
			"oaiapp_1",
			"nonce-1",
		),
		(
			id_token(
				json!({"iss":ISSUER,"aud":"oaiapp_1","sub":"u","exp":chrono::Utc::now().timestamp() - 600,"nonce":"nonce-1"}),
				"fixture",
			),
			"oaiapp_1",
			"nonce-1",
		),
		(
			id_token(claims("oaiapp_1", "nonce-1"), "unknown"),
			"oaiapp_1",
			"nonce-1",
		),
		("not-a-jwt".to_owned().into(), "oaiapp_1", "nonce-1"),
	] {
		assert!(
			authorization
				.verify_id_token(&token, client, nonce)
				.await
				.is_err()
		);
	}
}

/// Unreadable signing keys are a retryable outage, not a rejected
/// authorization.
#[tokio::test]
async fn unavailable_signing_keys_are_not_an_authentication_failure() {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	let discovery = json!({
		"issuer": ISSUER,
		"token_endpoint": format!("{url}/oauth/token"),
		"jwks_uri": format!("{url}/jwks"),
		"revocation_endpoint": format!("{url}/oauth/revoke"),
	});
	let app = Router::new()
		.route(
			"/.well-known/openid-configuration",
			get(move || async move { Json(discovery) }),
		)
		.route("/jwks", get(|| async { StatusCode::SERVICE_UNAVAILABLE }));
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let authorization = ChatgptAuthorization::at(
		reqwest::Client::new(),
		format!("{url}/.well-known/openid-configuration"),
		format!("{url}/v1"),
	);
	let token = id_token(claims("oaiapp_1", "nonce-1"), "fixture");
	let error = authorization
		.verify_id_token(&token, "oaiapp_1", "nonce-1")
		.await
		.unwrap_err();
	assert!(matches!(error, Error::External(_)), "{error:?}");
}

#[tokio::test]
async fn refreshes_keep_the_grant_and_classify_rejections() {
	let (authorization, seen, _server) = fake(
		(StatusCode::OK, tokens(domain::SCOPES)),
		StatusCode::OK,
		ok_models(),
	)
	.await;
	let issued = authorization
		.refresh("oaiapp_1", &"refresh-0".to_owned().into())
		.await
		.unwrap();
	assert_eq!(issued.refresh_token.expose_secret(), "refresh-canary");
	let form = seen.lock().unwrap().token_forms[0].clone();
	assert_eq!(form["grant_type"], "refresh_token");
	assert_eq!(form["client_id"], "oaiapp_1");
	assert_eq!(form["refresh_token"], "refresh-0");
	assert_eq!(form["resource"], "https://api.openai.com/v1");
	// Omitting `scope` keeps the grant.
	assert!(!form.contains_key("scope"));

	for (status, code, terminal) in [
		(StatusCode::BAD_REQUEST, "invalid_grant", true),
		(StatusCode::BAD_REQUEST, "refresh_token_reused", true),
		(StatusCode::UNAUTHORIZED, "invalid_client", false),
		(StatusCode::SERVICE_UNAVAILABLE, "server_error", false),
	] {
		let (authorization, _, _server) =
			fake((status, json!({"error":code})), StatusCode::OK, ok_models()).await;
		let result = authorization
			.refresh("oaiapp_1", &"refresh-0".to_owned().into())
			.await;
		if terminal {
			assert!(matches!(result, Err(RefreshFailure::Terminal)), "{code}");
		} else {
			assert!(
				matches!(result, Err(RefreshFailure::Unavailable(_))),
				"{code}"
			);
		}
	}
}

#[tokio::test]
async fn revocations_send_the_refresh_token_and_client() {
	let (authorization, seen, _server) = fake(
		(StatusCode::OK, tokens(domain::SCOPES)),
		StatusCode::OK,
		ok_models(),
	)
	.await;
	authorization
		.revoke("oaiapp_1", &"refresh-1".to_owned().into())
		.await
		.unwrap();
	let form = seen.lock().unwrap().revocations[0].clone();
	assert_eq!(form["token"], "refresh-1");
	assert_eq!(form["token_type_hint"], "refresh_token");
	assert_eq!(form["client_id"], "oaiapp_1");

	let (authorization, seen, _server) = fake(
		(StatusCode::OK, tokens(domain::SCOPES)),
		StatusCode::SERVICE_UNAVAILABLE,
		ok_models(),
	)
	.await;
	assert!(
		authorization
			.revoke("oaiapp_1", &"refresh-1".to_owned().into())
			.await
			.is_err()
	);
	// Transient failures are retried before giving up.
	assert_eq!(seen.lock().unwrap().revocations.len(), 3);
}

#[tokio::test]
async fn model_lists_keep_listed_entries_and_default_missing_limits() {
	let (authorization, seen, _server) = fake(
		(StatusCode::OK, tokens(domain::SCOPES)),
		StatusCode::OK,
		(
			StatusCode::OK,
			json!({"models":[
				{"slug":"gpt-fixture","display_name":"GPT Fixture","visibility":"list","context_window":400000,"max_output_tokens":128000},
				{"slug":"gpt-hidden","display_name":"Hidden","visibility":"hide"},
				{"slug":"gpt-mini","visibility":"list"},
				{"slug":"bad/../slug","display_name":"Bad","visibility":"list"}
			]}),
		),
	)
	.await;
	let models = authorization
		.models(&"access-1".to_owned().into())
		.await
		.unwrap();
	assert_eq!(seen.lock().unwrap().bearers, vec!["Bearer access-1"]);
	assert_eq!(
		models,
		vec![
			PlanModel {
				slug: "gpt-fixture".into(),
				display_name: "GPT Fixture".into(),
				context_window: 400_000,
				max_output_tokens: 128_000,
				defaulted: false,
			},
			PlanModel {
				slug: "gpt-mini".into(),
				display_name: "gpt-mini".into(),
				context_window: DEFAULT_CONTEXT_WINDOW,
				max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
				defaulted: true,
			},
		]
	);
}

#[tokio::test]
async fn model_list_rejections_use_the_plan_error_table() {
	let (authorization, _, _server) = fake(
		(StatusCode::OK, tokens(domain::SCOPES)),
		StatusCode::OK,
		(
			StatusCode::FORBIDDEN,
			json!({"error":{"code":"subscription_sharing_user_not_eligible","message":"canary"}}),
		),
	)
	.await;
	let error = authorization
		.models(&"access-1".to_owned().into())
		.await
		.unwrap_err();
	assert!(matches!(error, Error::Plan(domain::Failure::Ineligible)));
}

/// `earliest_refresh_at` has no documented format: epoch seconds and RFC 3339
/// are read, and anything else is ignored rather than failing the tokens.
#[test]
fn earliest_refresh_times_are_read_leniently() {
	let at = DateTime::from_timestamp(1_790_000_000, 0);
	assert_eq!(instant(&json!(1_790_000_000)), at);
	assert_eq!(instant(&json!("1790000000")), at);
	assert_eq!(instant(&json!("2026-09-21T14:13:20Z")), at);
	assert_eq!(instant(&json!("soon")), None);
	assert_eq!(instant(&json!(null)), None);
	let tokens: Tokens = serde_json::from_value(json!({
		"access_token":"a","refresh_token":"r","expires_in":3600,
		"earliest_refresh_at":1_790_000_000
	}))
	.unwrap();
	assert_eq!(tokens.into_issued().unwrap().earliest_refresh_at, at);
}

/// A malformed successful refresh response can follow upstream rotation.
/// Its uncertainty must not be classified as permission to replay the grant.
#[rstest::rstest]
#[case::malformed_success(StatusCode::OK)]
#[case::proxy_failure(StatusCode::BAD_GATEWAY)]
#[tokio::test]
async fn refresh_reply_loss_is_distinct_from_a_confirmed_rejection(#[case] status: StatusCode) {
	let (authorization, seen, _server) = fake(
		(status, json!({"access_token":"incomplete-response"})),
		StatusCode::OK,
		ok_models(),
	)
	.await;
	let result = authorization
		.refresh("oaiapp_1", &"refresh-0".to_owned().into())
		.await;
	assert_eq!(seen.lock().unwrap().token_forms.len(), 1);
	assert!(result.is_err());
	assert!(
		matches!(result, Err(RefreshFailure::Ambiguous(_))),
		"a possibly rotated grant cannot safely be retried"
	);
}

#[tokio::test]
async fn refresh_discovery_failure_is_safe_to_retry_before_dispatch() {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/discovery",
		get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
	);
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let authorization = ChatgptAuthorization::at(
		reqwest::Client::new(),
		format!("{url}/discovery"),
		format!("{url}/v1"),
	);
	assert!(matches!(
		authorization
			.refresh("oaiapp_1", &"refresh-0".to_owned().into())
			.await,
		Err(RefreshFailure::Unavailable(_))
	));
}
