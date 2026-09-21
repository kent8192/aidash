mod common;
use aidash::{api, domain::qualified_agent, federation::Peer};
use axum::{
	body::Body,
	extract::Request,
	middleware::{self, Next},
};
use common::*;
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn discovery_intersects_both_nodes_without_forwarding_subject_tokens() {
	let (a, a_url, a_schema) = setup().await;
	let (mut b, b_url, b_schema) = setup().await;
	b.config.node_id = "aidash://destination".into();
	b.store.node_id = b.config.node_id.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	b.config.endpoint = format!("http://{}", listener.local_addr().unwrap());
	let a_app = api::router(a.clone());
	let b_app = api::router(b.clone());
	let (mut policy, token, _) = bootstrap(&a, &a_app, "http://localhost:1").await;
	bootstrap(&b, &b_app, "http://localhost:1").await;
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("peers"))
			.columns([
				sea_orm::sea_query::Alias::new("node_id"),
				sea_orm::sea_query::Alias::new("endpoint"),
				sea_orm::sea_query::Alias::new("credential_env"),
				sea_orm::sea_query::Alias::new("protocol_version"),
				sea_orm::sea_query::Alias::new("enabled"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("'http://localhost:1'"),
				sea_orm::sea_query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
				sea_orm::sea_query::Expr::cust("'0.1'"),
				sea_orm::sea_query::Expr::cust("TRUE"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&a.config.node_id)
	.execute(&b.store.pool)
	.await
	.unwrap();
	let calls = Arc::new(AtomicUsize::new(0));
	let observed = calls.clone();
	let peer_token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
	let peer_app = b_app
		.clone()
		.layer(middleware::from_fn(move |request: Request, next: Next| {
			let observed = observed.clone();
			let peer_token = peer_token.clone();
			async move {
				if request.uri().path() == "/federation/v0.1/scoped/discover" {
					observed.fetch_add(1, Ordering::SeqCst);
					assert_eq!(
						request.headers()["authorization"],
						format!("Bearer {peer_token}")
					);
					let (parts, body) = request.into_parts();
					let bytes = axum::body::to_bytes(body, 1_048_576).await.unwrap();
					let body: Value = serde_json::from_slice(&bytes).unwrap();
					assert_eq!(body.as_object().unwrap().len(), 3);
					assert_eq!(body["tenant"], "acme");
					assert_eq!(body["subject"], "alice");
					next.run(Request::from_parts(parts, Body::from(bytes)))
						.await
				} else {
					next.run(request).await
				}
			}
		}));
	let server = tokio::spawn(async move {
		axum::serve(listener, peer_app).await.unwrap();
	});
	a.register_peer(Peer {
		node_id: b.config.node_id.clone(),
		endpoint: b.config.endpoint.clone(),
		credential_env: "AIDASH_SECRET_TEST_PEER".into(),
		protocol_version: "0.1".into(),
		enabled: true,
	})
	.await
	.unwrap();
	let (status, result) = request(&a_app, &token, "POST", "/api/discover", json!({})).await;
	assert_eq!(status, 200);
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(
		result["errors"][0]["error"],
		"authorized peer discovery unavailable"
	);
	let (_, issued) = request(
		&b_app,
		&b.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let mapping = json!({"source_node":a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":issued["credential"]["id"],"expected_revision":0,"enabled":true});
	assert_eq!(
		request(
			&b_app,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme/peer-mappings",
			mapping
		)
		.await
		.0,
		200
	);
	let (status, result) = request(&a_app, &token, "POST", "/api/discover", json!({})).await;
	assert_eq!(status, 200);
	assert_eq!(result["agents"].as_array().unwrap().len(), 2);
	assert_eq!(result["errors"], json!([]));
	// Same id/version on both nodes must not alias a source-side deny.
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"hide-remote","effect":"deny","subjects":{"any":true},"actions":["registry.read"],"resources":{"kinds":["agent"],"ids":[qualified_agent(&b.config.node_id,"research","1.0.0")]}}));
	assert_eq!(
		request(
			&a_app,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let (_, result) = request(&a_app, &token, "POST", "/api/discover", json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(result["agents"][0]["node_id"], a.config.node_id);
	policy["policies"].as_array_mut().unwrap().pop();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"no-disclosure","effect":"deny","subjects":{"any":true},"actions":["federation.discover"],"resources":{"kinds":["node"]}}));
	assert_eq!(
		request(
			&a_app,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let before = calls.load(Ordering::SeqCst);
	let (_, result) = request(&a_app, &token, "POST", "/api/discover", json!({})).await;
	assert_eq!(
		calls.load(Ordering::SeqCst),
		before,
		"denied discovery must not disclose tenant, subject or query"
	);
	assert_eq!(
		result["errors"],
		json!([]),
		"denied peers must not be listed"
	);
	policy["policies"].as_array_mut().unwrap().pop();
	assert_eq!(
		request(
			&a_app,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":3,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&b_app,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false})
		)
		.await
		.0,
		200
	);
	let (_, result) = request(&a_app, &token, "POST", "/api/discover", json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(result["errors"], json!([]));
	assert_eq!(
		request(
			&b_app,
			&b.config.api_token,
			"POST",
			&format!(
				"/api/authorization/acme/credentials/{}/revoke",
				issued["credential"]["id"].as_str().unwrap()
			),
			json!({})
		)
		.await
		.0,
		200
	);
	let (_, result) = request(&a_app, &token, "POST", "/api/discover", json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(result["errors"].as_array().unwrap().len(), 1);
	server.abort();
	let _ = server.await;
	cleanup(a, &a_url, &a_schema).await;
	cleanup(b, &b_url, &b_schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn in_flight_discovery_retains_source_authority_and_subsequent_requests_honor_revocation() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://localhost:1").await;
	let entry = f
		.registry
		.list(&aidash::registry::Search {
			kind: Some("agent".into()),
			..Default::default()
		})
		.await
		.unwrap()
		.remove(0);
	let started = Arc::new(tokio::sync::Notify::new());
	let release = Arc::new(tokio::sync::Notify::new());
	let ready = started.clone();
	let gate = release.clone();
	let peer = axum::Router::new().route(
		"/federation/v0.1/scoped/discover",
		axum::routing::post(move || {
			let ready = ready.clone();
			let gate = gate.clone();
			let entry = entry.clone();
			async move {
				ready.notify_one();
				gate.notified().await;
				axum::Json(vec![entry])
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, peer).await.unwrap();
	});
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("peers"))
			.columns([
				sea_orm::sea_query::Alias::new("node_id"),
				sea_orm::sea_query::Alias::new("endpoint"),
				sea_orm::sea_query::Alias::new("credential_env"),
				sea_orm::sea_query::Alias::new("protocol_version"),
				sea_orm::sea_query::Alias::new("enabled"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("'aidash://slow-peer'"),
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
				sea_orm::sea_query::Expr::cust("'0.1'"),
				sea_orm::sea_query::Expr::cust("TRUE"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(endpoint)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let active_app = app.clone();
	let active_token = token.clone();
	let active = tokio::spawn(async move {
		request(
			&active_app,
			&active_token,
			"POST",
			"/api/discover",
			json!({}),
		)
		.await
	});
	tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
		.await
		.unwrap();
	let credential: uuid::Uuid = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
			))
			.from(sea_orm::sea_query::Alias::new("authorization_credentials"))
			.and_where(sea_orm::sea_query::Expr::cust("subject = 'alice'"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	let revoker_app = app.clone();
	let operator = f.config.api_token.clone();
	let mut revoker = tokio::spawn(async move {
		request(
			&revoker_app,
			&operator,
			"POST",
			&format!("/api/authorization/acme/credentials/{credential}/revoke"),
			json!({}),
		)
		.await
	});
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(100), &mut revoker)
			.await
			.is_err(),
		"revocation must wait for the admitted boundary"
	);
	release.notify_one();
	let (status, result) = active.await.unwrap();
	assert_eq!(status, 200);
	assert_eq!(result["agents"].as_array().unwrap().len(), 2);
	assert_eq!(revoker.await.unwrap().0, 200);
	assert_eq!(
		request(&app, &token, "POST", "/api/discover", json!({}))
			.await
			.0,
		401
	);
	server.abort();
	let _ = server.await;
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn source_filters_search_results_and_rejects_invalid_peer_metadata() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://localhost:1").await;
	let entry = f
		.registry
		.list(&aidash::registry::Search {
			kind: Some("agent".into()),
			..Default::default()
		})
		.await
		.unwrap()
		.remove(0);
	let mode = Arc::new(AtomicUsize::new(0));
	let selected = mode.clone();
	let peer = axum::Router::new().route(
		"/federation/v0.1/scoped/discover",
		axum::routing::post(move || {
			let mode = selected.load(Ordering::SeqCst);
			let mut entry = entry.clone();
			async move {
				let entries = match mode {
					1 => vec![entry.clone(), entry],
					2 => {
						entry.kind = "tool".into();
						vec![entry]
					}
					3 => {
						entry.version = "not-a-version".into();
						vec![entry]
					}
					_ => vec![entry],
				};
				axum::Json(entries)
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, peer).await.unwrap();
	});
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("peers"))
			.columns([
				sea_orm::sea_query::Alias::new("node_id"),
				sea_orm::sea_query::Alias::new("endpoint"),
				sea_orm::sea_query::Alias::new("credential_env"),
				sea_orm::sea_query::Alias::new("protocol_version"),
				sea_orm::sea_query::Alias::new("enabled"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("'aidash://untrusted-peer'"),
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
				sea_orm::sea_query::Expr::cust("'0.1'"),
				sea_orm::sea_query::Expr::cust("TRUE"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(endpoint)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let (_, result) = request(
		&app,
		&token,
		"POST",
		"/api/discover",
		json!({"capability":"not-present"}),
	)
	.await;
	assert_eq!(result["agents"], json!([]));
	assert_eq!(result["errors"], json!([]));
	for selected in 1..=3 {
		mode.store(selected, Ordering::SeqCst);
		let (status, result) = request(&app, &token, "POST", "/api/discover", json!({})).await;
		assert_eq!(status, 200);
		assert_eq!(result["agents"].as_array().unwrap().len(), 1);
		assert_eq!(result["agents"][0]["node_id"], f.config.node_id);
		assert_eq!(
			result["errors"][0]["error"],
			"invalid peer discovery response"
		);
	}
	server.abort();
	let _ = server.await;
	cleanup(f, &url, &schema).await;
}
