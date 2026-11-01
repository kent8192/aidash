use super::*;
use axum::{
	Json, Router,
	extract::Form,
	http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
	response::{IntoResponse, Response},
	routing::{get, post},
};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}

/// A loopback listener and the origin its requests reach.
async fn bind() -> (String, tokio::net::TcpListener) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let base = format!("http://{}", listener.local_addr().unwrap());
	(base, listener)
}

fn serve(listener: tokio::net::TcpListener, app: Router) -> Server {
	Server(tokio::spawn(async move {
		let _ = axum::serve(listener, app).await;
	}))
}

#[derive(Default)]
struct Seen {
	initializes: Vec<Value>,
	accepts: Vec<String>,
	protocols: Vec<String>,
	registrations: Vec<Value>,
	token_forms: Vec<BTreeMap<String, String>>,
	revocations: Vec<BTreeMap<String, String>>,
}

fn prm(resource: &str) -> Value {
	json!({
		"resource": resource,
		"authorization_servers": ["https://auth.example.com"],
		"scopes_supported": ["mcp"],
	})
}

/// Authorization server metadata; `s256` false answers with `plain` only.
fn authorization_document(base: &str, issuer: &str, s256: bool) -> Value {
	json!({
		"issuer": issuer,
		"authorization_endpoint": format!("{base}/authorize"),
		"token_endpoint": format!("{base}/token"),
		"registration_endpoint": format!("{base}/register"),
		"revocation_endpoint": format!("{base}/revoke"),
		"code_challenge_methods_supported": if s256 { json!(["S256"]) } else { json!(["plain"]) },
	})
}

/// An authorization server metadata value as discovery would produce it.
fn discovered(base: &str) -> AuthorizationServer {
	AuthorizationServer {
		issuer: base.to_owned(),
		authorization_endpoint: Url::parse(&format!("{base}/authorize")).unwrap(),
		token_endpoint: Url::parse(&format!("{base}/token")).unwrap(),
		registration_endpoint: Some(Url::parse(&format!("{base}/register")).unwrap()),
		revocation_endpoint: Some(Url::parse(&format!("{base}/revoke")).unwrap()),
		scopes_supported: None,
		code_challenge_methods_supported: vec!["S256".into()],
	}
}

fn bearer(header_value: &str) -> Response {
	(
		StatusCode::UNAUTHORIZED,
		[(header::WWW_AUTHENTICATE, header_value.to_owned())],
	)
		.into_response()
}

fn client(secret: Option<&str>) -> ClientCredentials {
	ClientCredentials {
		client_id: "client-1".into(),
		client_secret: secret.map(|secret| SecretString::from(secret.to_owned())),
	}
}

fn verifier() -> SecretString {
	SecretString::from("verifier-canary".to_owned())
}

#[tokio::test]
async fn a_401_challenge_points_at_the_protected_resource_metadata() {
	let (base, listener) = bind().await;
	let seen = Arc::new(Mutex::new(Seen::default()));
	let app = Router::new()
		.route(
			"/mcp",
			post({
				let seen = seen.clone();
				let base = base.clone();
				move |headers: HeaderMap, Json(body): Json<Value>| {
					let seen = seen.clone();
					let base = base.clone();
					async move {
						{
							let mut seen = seen.lock().unwrap();
							seen.initializes.push(body);
							seen.accepts.push(
								headers
									.get("accept")
									.and_then(|value| value.to_str().ok())
									.unwrap_or_default()
									.to_owned(),
							);
							seen.protocols.push(
								headers
									.get("mcp-protocol-version")
									.and_then(|value| value.to_str().ok())
									.unwrap_or_default()
									.to_owned(),
							);
						}
						let mut response = bearer("Basic realm=\"mcp\"");
						response.headers_mut().append(
							header::WWW_AUTHENTICATE,
							HeaderValue::from_str(&format!(
								"Bearer resource_metadata=\"{base}/authorization/resource\""
							))
							.unwrap(),
						);
						response
					}
				}
			}),
		)
		.route(
			"/authorization/resource",
			get({
				let base = base.clone();
				move || async move {
					let mut document = prm(&format!("{base}/mcp"));
					document["authorization_servers"] = json!([format!("{base}/issuer")]);
					Json(document)
				}
			}),
		);
	let _server = serve(listener, app);
	let adapter = McpOAuth::for_test().unwrap();
	let metadata = adapter
		.protected_resource(&format!("{base}/mcp"))
		.await
		.unwrap()
		.unwrap();
	assert_eq!(metadata.resource, format!("{base}/mcp"));
	assert_eq!(
		metadata.authorization_servers,
		vec![format!("{base}/issuer")]
	);
	assert_eq!(
		metadata.scopes_supported.as_deref(),
		Some(&["mcp".to_owned()][..])
	);
	let seen = seen.lock().unwrap();
	assert_eq!(seen.initializes[0]["method"], "initialize");
	assert_eq!(
		seen.initializes[0]["params"]["protocolVersion"],
		PROTOCOL_VERSION
	);
	assert_eq!(seen.accepts[0], "application/json, text/event-stream");
	assert_eq!(seen.protocols[0], PROTOCOL_VERSION);
}

#[tokio::test]
async fn a_401_without_a_challenge_falls_back_to_the_path_well_known() {
	let (base, listener) = bind().await;
	let app = Router::new()
		.route("/mcp/server", post(|| async { StatusCode::UNAUTHORIZED }))
		.route(
			"/.well-known/oauth-protected-resource/mcp/server",
			get({
				let base = base.clone();
				move || async move { Json(prm(&format!("{base}/mcp/server"))) }
			}),
		);
	let _server = serve(listener, app);
	let metadata = McpOAuth::for_test()
		.unwrap()
		.protected_resource(&format!("{base}/mcp/server"))
		.await
		.unwrap()
		.unwrap();
	assert_eq!(metadata.resource, format!("{base}/mcp/server"));
}

#[tokio::test]
async fn a_401_without_a_challenge_falls_back_to_the_origin_well_known() {
	let (base, listener) = bind().await;
	let app = Router::new()
		.route("/mcp/server", post(|| async { StatusCode::UNAUTHORIZED }))
		.route(
			"/.well-known/oauth-protected-resource",
			get({
				let base = base.clone();
				move || async move { Json(prm(&format!("{base}/mcp/server"))) }
			}),
		);
	let _server = serve(listener, app);
	let metadata = McpOAuth::for_test()
		.unwrap()
		.protected_resource(&format!("{base}/mcp/server"))
		.await
		.unwrap()
		.unwrap();
	assert_eq!(metadata.resource, format!("{base}/mcp/server"));
}

#[tokio::test]
async fn a_server_that_initializes_without_a_token_needs_no_oauth() {
	let (base, listener) = bind().await;
	let app = Router::new().route(
		"/mcp",
		post(|| async { Json(json!({"jsonrpc": "2.0", "id": 1, "result": {}})) }),
	);
	let _server = serve(listener, app);
	let metadata = McpOAuth::for_test()
		.unwrap()
		.protected_resource(&format!("{base}/mcp"))
		.await
		.unwrap();
	assert!(metadata.is_none());
}

#[tokio::test]
async fn a_metadata_document_for_another_resource_is_refused() {
	let (base, listener) = bind().await;
	let app = Router::new()
		.route(
			"/mcp",
			post({
				let base = base.clone();
				move || async move {
					bearer(&format!(
						"Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\""
					))
				}
			}),
		)
		.route(
			"/.well-known/oauth-protected-resource",
			get({
				let base = base.clone();
				move || async move { Json(prm(&format!("{base}/another"))) }
			}),
		);
	let _server = serve(listener, app);
	let error = McpOAuth::for_test()
		.unwrap()
		.protected_resource(&format!("{base}/mcp"))
		.await
		.err()
		.expect("metadata must be refused");
	assert!(matches!(error, Error::Invalid(_)));
}

#[tokio::test]
async fn rfc8414_path_insertion_is_used_for_an_issuer_with_a_path() {
	let (base, listener) = bind().await;
	let issuer = format!("{base}/tenant1");
	let seen = Arc::new(Mutex::new(Vec::new()));
	let app = Router::new().route(
		"/.well-known/oauth-authorization-server/tenant1",
		get({
			let base = base.clone();
			let issuer = issuer.clone();
			let seen = seen.clone();
			move |uri: Uri| {
				let base = base.clone();
				let issuer = issuer.clone();
				let seen = seen.clone();
				async move {
					seen.lock().unwrap().push(uri.path().to_owned());
					Json(authorization_document(&base, &issuer, true))
				}
			}
		}),
	);
	let _server = serve(listener, app);
	let server = McpOAuth::for_test()
		.unwrap()
		.authorization_server(&issuer)
		.await
		.unwrap();
	assert_eq!(server.issuer, issuer);
	assert_eq!(
		server.token_endpoint,
		Url::parse(&format!("{base}/token")).unwrap()
	);
	assert_eq!(
		*seen.lock().unwrap(),
		vec!["/.well-known/oauth-authorization-server/tenant1".to_owned()]
	);
}

#[tokio::test]
async fn openid_discovery_is_the_fallback() {
	let (base, listener) = bind().await;
	let issuer = format!("{base}/tenant1");
	let app = Router::new().route(
		"/tenant1/.well-known/openid-configuration",
		get({
			let base = base.clone();
			let issuer = issuer.clone();
			move || async move { Json(authorization_document(&base, &issuer, true)) }
		}),
	);
	let _server = serve(listener, app);
	let server = McpOAuth::for_test()
		.unwrap()
		.authorization_server(&issuer)
		.await
		.unwrap();
	assert_eq!(server.issuer, issuer);
}

#[tokio::test]
async fn another_issuer_is_refused() {
	let (base, listener) = bind().await;
	let app = Router::new().route(
		"/.well-known/oauth-authorization-server",
		get({
			let base = base.clone();
			let other = format!("{base}/other");
			move || async move { Json(authorization_document(&base, &other, true)) }
		}),
	);
	let _server = serve(listener, app);
	let error = McpOAuth::for_test()
		.unwrap()
		.authorization_server(&base)
		.await
		.err()
		.expect("metadata must be refused");
	assert!(matches!(error, Error::Invalid(_)));
}

#[tokio::test]
async fn a_server_without_s256_is_refused() {
	for methods in [None, Some(json!([])), Some(json!(["plain"]))] {
		let (base, listener) = bind().await;
		let mut document = authorization_document(&base, &base, false);
		if let Some(methods) = methods {
			document["code_challenge_methods_supported"] = methods;
		} else {
			document
				.as_object_mut()
				.unwrap()
				.remove("code_challenge_methods_supported");
		}
		let app = Router::new().route(
			"/.well-known/oauth-authorization-server",
			get(move || async move { Json(document) }),
		);
		let _server = serve(listener, app);
		assert!(matches!(
			McpOAuth::for_test()
				.unwrap()
				.authorization_server(&base)
				.await,
			Err(Error::Invalid(_))
		));
	}
}

#[tokio::test]
async fn dynamic_registration_accepts_a_secret_or_a_public_client() {
	for secret in [Some("secret-canary".to_owned()), None] {
		let (base, listener) = bind().await;
		let seen = Arc::new(Mutex::new(Seen::default()));
		let app = Router::new().route(
			"/register",
			post({
				let seen = seen.clone();
				let secret = secret.clone();
				move |Json(body): Json<Value>| {
					let seen = seen.clone();
					let secret = secret.clone();
					async move {
						seen.lock().unwrap().registrations.push(body);
						let mut reply = json!({"client_id": "client-1"});
						if let Some(secret) = &secret {
							reply["client_secret"] = json!(secret);
						}
						Json(reply)
					}
				}
			}),
		);
		let _server = serve(listener, app);
		let redirect = Url::parse("https://app.example/auth/connections/callback").unwrap();
		let credentials = McpOAuth::for_test()
			.unwrap()
			.register(&discovered(&base), &redirect, "Aidash")
			.await
			.unwrap();
		assert_eq!(credentials.client_id, "client-1");
		assert_eq!(
			credentials
				.client_secret
				.as_ref()
				.map(ExposeSecret::expose_secret),
			secret.as_deref()
		);
		let registration = &seen.lock().unwrap().registrations[0];
		assert_eq!(registration["client_name"], "Aidash");
		assert_eq!(
			registration["redirect_uris"],
			json!(["https://app.example/auth/connections/callback"])
		);
		assert_eq!(
			registration["grant_types"],
			json!(["authorization_code", "refresh_token"])
		);
		assert_eq!(registration["response_types"], json!(["code"]));
		assert_eq!(
			registration["token_endpoint_auth_method"],
			"client_secret_post"
		);
	}
}

#[tokio::test]
async fn an_exchange_sends_the_resource_verifier_and_client_secret() {
	let (base, listener) = bind().await;
	let seen = Arc::new(Mutex::new(Seen::default()));
	let app = Router::new().route(
		"/token",
		post({
			let seen = seen.clone();
			move |Form(form): Form<BTreeMap<String, String>>| {
				let seen = seen.clone();
				async move {
					seen.lock().unwrap().token_forms.push(form);
					Json(json!({
						"access_token": "access-canary",
						"refresh_token": "refresh-canary",
						"token_type": "Bearer",
						"expires_in": 3600,
						"scope": "mcp",
					}))
				}
			}
		}),
	);
	let _server = serve(listener, app);
	let redirect = Url::parse("https://app.example/auth/connections/callback").unwrap();
	let tokens = McpOAuth::for_test()
		.unwrap()
		.exchange(
			&discovered(&base),
			&client(Some("secret-canary")),
			"code-canary",
			&verifier(),
			&redirect,
			"https://mcp.example.com/mcp",
		)
		.await
		.unwrap();
	assert_eq!(tokens.access_token.expose_secret(), "access-canary");
	assert_eq!(
		tokens
			.refresh_token
			.as_ref()
			.map(ExposeSecret::expose_secret),
		Some("refresh-canary")
	);
	assert_eq!(tokens.expires_in, Some(3600));
	assert_eq!(tokens.scope.as_deref(), Some("mcp"));
	let form = &seen.lock().unwrap().token_forms[0];
	assert_eq!(form["grant_type"], "authorization_code");
	assert_eq!(form["code"], "code-canary");
	assert_eq!(form["code_verifier"], "verifier-canary");
	assert_eq!(
		form["redirect_uri"],
		"https://app.example/auth/connections/callback"
	);
	assert_eq!(form["resource"], "https://mcp.example.com/mcp");
	assert_eq!(form["client_id"], "client-1");
	assert_eq!(form["client_secret"], "secret-canary");
}

#[tokio::test]
async fn a_public_client_sends_only_its_client_id() {
	let (base, listener) = bind().await;
	let seen = Arc::new(Mutex::new(Seen::default()));
	let app = Router::new().route(
		"/token",
		post({
			let seen = seen.clone();
			move |Form(form): Form<BTreeMap<String, String>>| {
				let seen = seen.clone();
				async move {
					seen.lock().unwrap().token_forms.push(form);
					Json(json!({"access_token": "access-canary"}))
				}
			}
		}),
	);
	let _server = serve(listener, app);
	let redirect = Url::parse("https://app.example/auth/connections/callback").unwrap();
	McpOAuth::for_test()
		.unwrap()
		.exchange(
			&discovered(&base),
			&client(None),
			"code-canary",
			&verifier(),
			&redirect,
			"https://mcp.example.com/mcp",
		)
		.await
		.unwrap();
	let form = &seen.lock().unwrap().token_forms[0];
	assert_eq!(form["client_id"], "client-1");
	assert!(!form.contains_key("client_secret"));
}

#[tokio::test]
async fn invalid_grant_ends_the_grant() {
	let (base, listener) = bind().await;
	let app = Router::new().route(
		"/token",
		post(|| async {
			(
				StatusCode::BAD_REQUEST,
				Json(json!({"error": "invalid_grant"})),
			)
		}),
	);
	let _server = serve(listener, app);
	let result = McpOAuth::for_test()
		.unwrap()
		.refresh(
			&discovered(&base),
			&client(Some("secret-canary")),
			&SecretString::from("refresh-canary".to_owned()),
			"https://mcp.example.com/mcp",
		)
		.await;
	assert!(matches!(result, Err(RefreshFailure::Terminal)));
}

#[tokio::test]
async fn a_server_error_keeps_the_tokens() {
	let (base, listener) = bind().await;
	let app = Router::new().route(
		"/token",
		post(|| async {
			(
				StatusCode::SERVICE_UNAVAILABLE,
				Json(json!({"error": "temporarily_unavailable"})),
			)
		}),
	);
	let _server = serve(listener, app);
	let result = McpOAuth::for_test()
		.unwrap()
		.refresh(
			&discovered(&base),
			&client(None),
			&SecretString::from("refresh-canary".to_owned()),
			"https://mcp.example.com/mcp",
		)
		.await;
	assert!(matches!(result, Err(RefreshFailure::Unavailable(_))));
}

#[tokio::test]
async fn an_unparsable_refresh_reply_is_ambiguous() {
	let (base, listener) = bind().await;
	let app = Router::new().route("/token", post(|| async { "not a token response" }));
	let _server = serve(listener, app);
	let result = McpOAuth::for_test()
		.unwrap()
		.refresh(
			&discovered(&base),
			&client(None),
			&SecretString::from("refresh-canary".to_owned()),
			"https://mcp.example.com/mcp",
		)
		.await;
	assert!(matches!(result, Err(RefreshFailure::Ambiguous(_))));
}

#[tokio::test]
async fn revocation_posts_the_token() {
	let (base, listener) = bind().await;
	let seen = Arc::new(Mutex::new(Seen::default()));
	let app = Router::new().route(
		"/revoke",
		post({
			let seen = seen.clone();
			move |Form(form): Form<BTreeMap<String, String>>| {
				let seen = seen.clone();
				async move {
					seen.lock().unwrap().revocations.push(form);
					StatusCode::OK
				}
			}
		}),
	);
	let _server = serve(listener, app);
	McpOAuth::for_test()
		.unwrap()
		.revoke(
			&discovered(&base),
			&client(Some("secret-canary")),
			&SecretString::from("refresh-canary".to_owned()),
		)
		.await
		.unwrap();
	{
		let seen = seen.lock().unwrap();
		let form = &seen.revocations[0];
		assert_eq!(form["token"], "refresh-canary");
		assert_eq!(form["client_id"], "client-1");
		assert_eq!(form["client_secret"], "secret-canary");
	}

	let mut without = discovered(&base);
	without.revocation_endpoint = None;
	McpOAuth::for_test()
		.unwrap()
		.revoke(&without, &client(None), &verifier())
		.await
		.unwrap();
}

#[tokio::test]
async fn an_oversized_metadata_body_is_refused() {
	let (base, listener) = bind().await;
	let app = Router::new()
		.route(
			"/mcp",
			post({
				let base = base.clone();
				move || async move {
					bearer(&format!(
						"Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\""
					))
				}
			}),
		)
		.route(
			"/.well-known/oauth-protected-resource",
			get({
				let base = base.clone();
				move || async move {
					Json(json!({
						"resource": format!("{base}/mcp"),
						"authorization_servers": ["https://auth.example.com"],
						"scopes_supported": ["x".repeat(70 * 1024)],
					}))
				}
			}),
		);
	let _server = serve(listener, app);
	let error = McpOAuth::for_test()
		.unwrap()
		.protected_resource(&format!("{base}/mcp"))
		.await
		.err()
		.expect("metadata must be refused");
	assert!(matches!(error, Error::External(_)));
}

#[tokio::test]
async fn production_refuses_private_destinations_before_any_request() {
	let (base, listener) = bind().await;
	let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
	let app = Router::new().fallback({
		let calls = calls.clone();
		move || {
			let calls = calls.clone();
			async move {
				calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
				Json(json!({"client_id": "client-1", "access_token": "access-canary"}))
			}
		}
	});
	let _server = serve(listener, app);
	let adapter = McpOAuth::new().unwrap();
	let redirect = Url::parse("https://app.example/callback").unwrap();
	// Literal IPs bypass reqwest's DNS resolver. Hostnames exercise PublicDns.
	for origin in [
		base.clone(),
		base.replace("http:", "https:"),
		base.replace("http://127.0.0.1", "https://localhost"),
	] {
		let server = discovered(&origin);
		assert!(matches!(
			adapter.protected_resource(&format!("{origin}/mcp")).await,
			Err(Error::Forbidden)
		));
		assert!(matches!(
			adapter.authorization_server(&origin).await,
			Err(Error::Forbidden)
		));
		assert!(matches!(
			adapter.register(&server, &redirect, "Aidash").await,
			Err(Error::Forbidden)
		));
		assert!(matches!(
			adapter
				.exchange(
					&server,
					&client(None),
					"code",
					&verifier(),
					&redirect,
					"https://mcp.example/mcp"
				)
				.await,
			Err(Error::Forbidden)
		));
		assert!(matches!(
			adapter
				.refresh(
					&server,
					&client(None),
					&verifier(),
					"https://mcp.example/mcp"
				)
				.await,
			Err(RefreshFailure::Unavailable(Error::Forbidden))
		));
		assert!(matches!(
			adapter.revoke(&server, &client(None), &verifier()).await,
			Err(Error::Forbidden)
		));
	}
	assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn token_operations_refuse_endpoints_with_inline_credentials() {
	let (base, listener) = bind().await;
	let app = Router::new().fallback(|| async {
		Json(json!({"client_id": "client-1", "access_token": "access-canary"}))
	});
	let _server = serve(listener, app);
	let adapter = McpOAuth::for_test().unwrap();
	let server = discovered(&base.replace("http://", "http://person:secret-canary@"));
	let redirect = Url::parse("https://app.example/callback").unwrap();
	assert!(matches!(
		adapter.register(&server, &redirect, "Aidash").await,
		Err(Error::Invalid(_))
	));
	assert!(matches!(
		adapter
			.exchange(
				&server,
				&client(None),
				"code",
				&verifier(),
				&redirect,
				"https://mcp.example/mcp"
			)
			.await,
		Err(Error::Invalid(_))
	));
	assert!(matches!(
		adapter
			.refresh(
				&server,
				&client(None),
				&verifier(),
				"https://mcp.example/mcp"
			)
			.await,
		Err(RefreshFailure::Unavailable(Error::Invalid(_)))
	));
	assert!(matches!(
		adapter.revoke(&server, &client(None), &verifier()).await,
		Err(Error::Invalid(_))
	));
}

#[tokio::test]
async fn initialize_preserves_the_configured_resource_path() {
	let (base, listener) = bind().await;
	let app = Router::new().route("/mcp/", post(|| async { StatusCode::OK }));
	let _server = serve(listener, app);
	assert!(
		McpOAuth::for_test()
			.unwrap()
			.protected_resource(&format!("{base}/mcp/"))
			.await
			.unwrap()
			.is_none()
	);
}

#[tokio::test]
async fn a_refresh_sends_the_bound_resource_and_returns_the_rotated_grant() {
	let (base, listener) = bind().await;
	let seen = Arc::new(Mutex::new(Seen::default()));
	let app = Router::new().route("/token", post({
        let seen = seen.clone();
        move |Form(form): Form<BTreeMap<String, String>>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().token_forms.push(form);
                Json(json!({"access_token": "new-access", "refresh_token": "rotated-refresh", "expires_in": 1800, "scope": "mcp:read", "token_type": "Bearer"}))
            }
        }
    }));
	let _server = serve(listener, app);
	let tokens = McpOAuth::for_test()
		.unwrap()
		.refresh(
			&discovered(&base),
			&client(Some("secret-canary")),
			&SecretString::from("old-refresh".to_owned()),
			"https://mcp.example.com/mcp",
		)
		.await
		.unwrap();
	assert_eq!(tokens.access_token.expose_secret(), "new-access");
	assert_eq!(
		tokens
			.refresh_token
			.as_ref()
			.map(ExposeSecret::expose_secret),
		Some("rotated-refresh")
	);
	assert_eq!(tokens.expires_in, Some(1800));
	assert_eq!(tokens.scope.as_deref(), Some("mcp:read"));
	let form = &seen.lock().unwrap().token_forms[0];
	assert_eq!(form["grant_type"], "refresh_token");
	assert_eq!(form["refresh_token"], "old-refresh");
	assert_eq!(form["resource"], "https://mcp.example.com/mcp");
	assert_eq!(form["client_id"], "client-1");
	assert_eq!(form["client_secret"], "secret-canary");
	assert!(!form.contains_key("code_verifier"));
}

#[tokio::test]
async fn refresh_distinguishes_a_connect_failure_from_a_timeout_after_sending() {
	let (base, listener) = bind().await;
	drop(listener);
	let adapter = McpOAuth::for_test().unwrap();
	let result = adapter
		.refresh(
			&discovered(&base),
			&client(None),
			&verifier(),
			"https://mcp.example/mcp",
		)
		.await;
	assert!(matches!(result, Err(RefreshFailure::Unavailable(_))));

	let (base, listener) = bind().await;
	let received = Arc::new(std::sync::atomic::AtomicBool::new(false));
	let app = Router::new().route(
		"/token",
		post({
			let received = received.clone();
			move |Form(_): Form<BTreeMap<String, String>>| {
				let received = received.clone();
				async move {
					received.store(true, std::sync::atomic::Ordering::SeqCst);
					tokio::time::sleep(Duration::from_secs(11)).await;
					Json(json!({"access_token": "new-access"}))
				}
			}
		}),
	);
	let _server = serve(listener, app);
	let result = adapter
		.refresh(
			&discovered(&base),
			&client(None),
			&verifier(),
			"https://mcp.example/mcp",
		)
		.await;
	assert!(received.load(std::sync::atomic::Ordering::SeqCst));
	assert!(matches!(result, Err(RefreshFailure::Ambiguous(_))));
}

#[tokio::test]
async fn redirects_are_not_followed_by_a_token_request() {
	let (base, listener) = bind().await;
	let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
	let app = Router::new()
		.route(
			"/token",
			post(|| async {
				(
					StatusCode::TEMPORARY_REDIRECT,
					[(header::LOCATION, "/leak")],
				)
			}),
		)
		.route(
			"/leak",
			post({
				let reached = reached.clone();
				move || {
					let reached = reached.clone();
					async move {
						reached.store(true, std::sync::atomic::Ordering::SeqCst);
						Json(json!({"access_token": "new-access"}))
					}
				}
			}),
		);
	let _server = serve(listener, app);
	let result = McpOAuth::for_test()
		.unwrap()
		.refresh(
			&discovered(&base),
			&client(None),
			&verifier(),
			"https://mcp.example/mcp",
		)
		.await;
	assert!(matches!(result, Err(RefreshFailure::Unavailable(_))));
	assert!(!reached.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn oversized_streamed_token_responses_are_refused_without_leaking_the_body() {
	let (base, listener) = bind().await;
	let app = Router::new().route(
		"/token",
		post(|| async {
			let document = json!({"access_token": "token-canary".repeat(7000)}).to_string();
			let stream = tokio_stream::iter(
				document
					.as_bytes()
					.chunks(4096)
					.map(|chunk| Ok::<_, std::io::Error>(chunk.to_vec()))
					.collect::<Vec<_>>(),
			);
			axum::body::Body::from_stream(stream)
		}),
	);
	let _server = serve(listener, app);
	let adapter = McpOAuth::for_test().unwrap();
	let redirect = Url::parse("https://app.example/callback").unwrap();
	let error = adapter
		.exchange(
			&discovered(&base),
			&client(None),
			"code-canary",
			&verifier(),
			&redirect,
			"https://mcp.example/mcp",
		)
		.await
		.err()
		.expect("oversized token response must be refused");
	assert!(matches!(error, Error::External(_)));
	assert!(!error.to_string().contains("token-canary"));
	let result = adapter
		.refresh(
			&discovered(&base),
			&client(None),
			&verifier(),
			"https://mcp.example/mcp",
		)
		.await;
	assert!(matches!(result, Err(RefreshFailure::Ambiguous(_))));
}

#[tokio::test]
async fn discovery_refuses_insecure_authorization_endpoints() {
	for field in [
		"authorization_endpoint",
		"token_endpoint",
		"registration_endpoint",
		"revocation_endpoint",
	] {
		let (base, listener) = bind().await;
		let mut document = authorization_document(&base, &base, true);
		document[field] = json!("http://as.example.com/endpoint");
		let app = Router::new().route(
			"/.well-known/oauth-authorization-server",
			get(move || async move { Json(document) }),
		);
		let _server = serve(listener, app);
		assert!(
			matches!(
				McpOAuth::for_test()
					.unwrap()
					.authorization_server(&base)
					.await,
				Err(Error::Invalid(_))
			),
			"insecure {field} must be refused"
		);
	}
}
