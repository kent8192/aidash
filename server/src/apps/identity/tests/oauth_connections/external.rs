//! One PostgreSQL-backed grant, one refresh, and two independent callers on the wire.
use super::{common, personal_credentials};
use aidash_application::{
	Result,
	egress::{ExternalAdmission, ScopeAuthority, ScopeSettings},
	oauth_clients::Put,
	tenant_secrets::Ownership,
};
use aidash_domain::{egress::ScopeId, oauth_connections::OAuthProvider};
use aidash_server::{
	apps::execution::repositories::scope_egress::{DurableScopes, ScratchAuthority},
	federation::Home,
	tool::{PluginTool, Tool, ToolContext},
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::{
	Mock, MockServer, Request, ResponseTemplate,
	matchers::{header, method, path},
};

// The fixture authenticates the test proxy credential. All service admission,
// chain-root resolution, grant checks and accounting are the native implementation.
struct Authority {
	inner: ScratchAuthority,
	id: ScopeId,
	run: Uuid,
}
#[async_trait]
impl ScopeAuthority for Authority {
	async fn authenticate(&self, credential: &str) -> Result<ScopeSettings> {
		if credential != "fixture" {
			return Err(aidash_application::Error::Unauthorized);
		}
		let mut scope = self.inner.current(&self.id).await?;
		scope.run_id = Some(self.run);
		Ok(scope)
	}
	async fn current(&self, id: &ScopeId) -> Result<ScopeSettings> {
		self.inner.current(id).await
	}
	async fn admit_external(
		&self,
		scope: &ScopeSettings,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> Result<ExternalAdmission> {
		self.inner
			.admit_external(scope, service, method, path, digest)
			.await
	}
}

// Terminate TLS in front of wiremock so registration exercises the production
// HTTPS requirement and both callers reach the same protected resource.
async fn tls_upstream(mock: &MockServer) -> (String, reqwest::Client, tokio::task::JoinHandle<()>) {
	use tokio_rustls::{
		TlsAcceptor,
		rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer},
	};
	let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
	let der = cert.cert.der().clone();
	let config = ServerConfig::builder_with_provider(Arc::new(
		tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
	))
	.with_safe_default_protocol_versions()
	.unwrap()
	.with_no_client_auth()
	.with_single_cert(
		vec![der.clone()],
		PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
	)
	.unwrap();
	let acceptor = TlsAcceptor::from(Arc::new(config));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("https://{}", listener.local_addr().unwrap());
	let upstream = *mock.address();
	let server = tokio::spawn(async move {
		let mut jobs = tokio::task::JoinSet::new();
		loop {
			let (socket, _) = listener.accept().await.unwrap();
			let acceptor = acceptor.clone();
			jobs.spawn(async move {
				let mut secure = acceptor.accept(socket).await.unwrap();
				let mut plain = tokio::net::TcpStream::connect(upstream).await.unwrap();
				let _ = tokio::io::copy_bidirectional(&mut secure, &mut plain).await;
			});
		}
	});
	let client = reqwest::Client::builder()
		.no_proxy()
		.redirect(reqwest::redirect::Policy::none())
		.tls_certs_only([reqwest::Certificate::from_der(&der).unwrap()])
		.build()
		.unwrap();
	(origin, client, server)
}
#[rstest::rstest]
#[tokio::test]
async fn configured_oauth_refreshes_once_for_external_service_and_http_tool(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (mut f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let mock = MockServer::start().await;
	let issuer = mock.uri();
	let (origin, upstream, tls) = tls_upstream(&mock).await;
	let resource = format!("{origin}/api");
	Mock::given(method("POST"))
		.and(path("/token"))
		.and(header("authorization", "Basic Y2xpZW50OnNlY3JldA=="))
		.respond_with(|request: &Request| {
			let body = String::from_utf8_lossy(&request.body);
			if body.contains("refresh_token=rotated") {
				return ResponseTemplate::new(400).set_body_json(json!({"error":"invalid_grant"}));
			}
			let (token, ttl) = if body.contains("grant_type=authorization_code") {
				assert!(body.contains("code_verifier="));
				("old", 30)
			} else {
				assert!(
					body.contains("grant_type=refresh_token")
						&& body.contains("refresh_token=rotating")
				);
				("fresh", 3600)
			};
			assert!(!body.contains("resource="));
			ResponseTemplate::new(200).set_body_json(
				json!({"token_type":"Bearer","access_token":token,"refresh_token":if token=="old" {"rotating"}else{"rotated"},"expires_in":ttl}),
			)
		})
		.expect(3)
		.mount(&mock)
		.await;
	for (verb, route) in [("GET", "/api/service"), ("POST", "/api/tool")] {
		Mock::given(method(verb))
			.and(path(route))
			.and(header("authorization", "Bearer fresh"))
			.respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
			.expect(1)
			.mount(&mock)
			.await;
	}
	let (_, token, _) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, result) = common::post(
		&app,
		&token,
		"/api/threads",
		json!({
			"title": "OAuth",
			"request": "Call the service",
			"target": {"id":"research","version":"1.0.0"},
			"target_kind": "agent"
		}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let scope = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
	let run = f.store.runs(&scope).await.unwrap().pop().unwrap();
	let owner = aidash_server::apps::identity::repositories::oauth_connections::owner_of_run(
		&f.store, &run,
	)
	.await
	.unwrap()
	.unwrap();
	personal_credentials::configure(&mut f).await;
	let oauth = Arc::new(aidash_integrations::oauth::McpOAuth::for_test().unwrap());
	aidash_server::bootstrap::configure_oauth_connections(
		&mut f.store,
		Some((oauth.clone(), oauth)),
		&serde_json::from_value(json!({})).unwrap(),
		aidash_server::bootstrap::ProcessRole::Server,
	)
	.unwrap();
	personal_credentials::clients(&f)
		.put(
			"acme",
			&owner.subject,
			None,
			Put {
				owner: Ownership::Me,
				issuer: issuer.clone(),
				client_id: "client".into(),
				client_secret: Some("secret".into()),
				expected_revision: 0,
				provider: Some(OAuthProvider {
					authorization_endpoint: format!("{issuer}/authorize"),
					token_endpoint: format!("{issuer}/token"),
					resource: resource.clone(),
					scopes: vec!["offline.access".into()],
				}),
			},
		)
		.await
		.unwrap();
	let connection = f.store.oauth_connections.clone().unwrap();
	let node = aidash_domain::nodes::NodeName::derived("acme");
	f.store.keyless.permit_origin_of(&origin).unwrap();
	let entry = json!({"id":"fixture","kind":"external_service","version":"1.0.0","name":{"en":"Fixture"},"description":{"en":"Local HTTPS service"},"config":{"base_url":format!("{resource}/"),"methods":["GET"],"oauth_connection":resource}});
	f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			serde_json::from_value(entry).unwrap(),
		)
		.await
		.unwrap();
	let authorization = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	authorization
		.set_catalog(
			"acme",
			&node,
			&aidash_domain::registry::EntityRef {
				id: "fixture".into(),
				version: "1.0.0".into(),
			},
			0,
			true,
			"deployment_administrator",
		)
		.await
		.unwrap();
	let settings = aidash_domain::scratch::ScratchSettings {
		external_services: ["fixture".to_owned()].into(),
		..Default::default()
	};
	let (status, created) = common::post(
		&app,
		&token,
		"/api/scratch-spaces",
		json!({"node":node,"name":"OAuth calls","settings":settings}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let space: Uuid = serde_json::from_value(created["id"].clone()).unwrap();
	let grant = connection
		.begin_configured(
			&owner,
			&issuer,
			Some(space),
			&resource,
			&[],
			&"https://aidash.example/callback".parse().unwrap(),
			"/",
		)
		.await
		.unwrap();
	assert!(
		grant
			.authorization_url
			.contains("code_challenge_method=S256")
	);
	assert!(grant.authorization_url.contains("scope=offline.access"));
	assert_eq!(
		connection
			.complete(grant.pending, Some("code"), None)
			.await
			.unwrap(),
		aidash_application::oauth_connections::Outcome::Connected
	);
	assert!(
		connection
			.space_bearer(&owner, &resource, Uuid::new_v4())
			.await
			.is_err()
	);

	let config = json!({"transport":"http","endpoint":format!("{resource}/tool"),"replay":"idempotent","oauth_connection":resource});
	let entry = json!({
	   "id":"fixture-http","version":"1.0.0","kind":"tool","name":{"en":"Fixture"},
	   "description":{"en":"Fixture"},"schema":{"type":"object"},
	   "config":{"provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","transport":config}
	});
	let tool = PluginTool {
		entry: serde_json::from_value(entry).unwrap(),
		alias: "fixture".into(),
		config: serde_json::from_value(config).unwrap(),
		transport: aidash_integrations::tools::HttpTools::new(
			f.store.keyless.clone(),
			f.store
				.tool_clients
				.clone()
				.with_test_client(upstream.clone()),
		),
	};
	let authority = Authority {
		inner: ScratchAuthority::new(f.store.clone(), f.config.scratch_policy.clone()),
		id: ScopeId {
			tenant: "acme".into(),
			node: node.to_string(),
			scratch_space: space,
		},
		run: run.id,
	};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let proxy = reqwest::Proxy::http(format!("http://{}", listener.local_addr().unwrap()))
		.unwrap()
		.basic_auth("aidash", "fixture");
	let client = reqwest::Client::builder().proxy(proxy).build().unwrap();
	let (stop, receiver) = tokio::sync::watch::channel(false);
	let gateway = tokio::spawn(aidash_integrations::egress::serve_with_test_client(
		listener,
		Arc::new(DurableScopes::new(f.store.clone(), Arc::new(authority))),
		receiver,
		upstream,
	));
	let context = ToolContext {
		home: Home::new(f.clone(), run.clone()),
		store: f.store.clone(),
		run,
		scope,
	};
	let (service, tool_result) = tokio::join!(
		client
			.get("http://services.aidash.internal/fixture/service")
			.send(),
		tool.invoke(&context, json!({}), "oauth-test")
	);
	let service = service.unwrap();
	assert_eq!(service.status(), 200, "{}", service.text().await.unwrap());
	assert_eq!(tool_result.unwrap(), json!({"ok":true}));
	connection.invalidate(&owner, &resource).await.unwrap();
	let refused = client
		.get("http://services.aidash.internal/fixture/service")
		.send()
		.await
		.unwrap();
	assert_eq!(refused.status(), reqwest::StatusCode::FAILED_DEPENDENCY);
	let failure: serde_json::Value = refused.json().await.unwrap();
	let hint = aidash_domain::oauth_connections::reconnect(&resource);
	assert_eq!(
		failure,
		json!({"error":"connection_required","resource":resource,"hint":hint})
	);
	let tool_error = tool
		.invoke(&context, json!({}), "oauth-reconnect")
		.await
		.unwrap_err();
	assert!(tool_error.to_string().contains(&hint), "{tool_error}");
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
	tls.abort();
	mock.verify().await;
	drop(context);
	common::cleanup(f, &url, &schema).await;
}
