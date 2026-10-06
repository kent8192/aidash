//! Outbound discovery uses both nodes' current authority over native HTTP.
#[path = "../../../execution/tests/support/endpoint.rs"]
mod endpoint;
#[path = "../../../execution/tests/support/execution.rs"]
mod execution_fixtures;
#[path = "../../../execution/tests/support/isolated.rs"]
mod isolated;
#[path = "../../../execution/tests/support/native_database.rs"]
mod native_database;
// The successful provider fixture also serves other adapter test variants.
mod discovery_fixtures;
#[allow(dead_code)]
#[path = "../../../execution/tests/provider_fixtures.rs"]
mod provider_fixtures;

use aidash_server::{apps::identity::models::AuthorizationCredential, domain::qualified_agent};
use discovery_fixtures::{
	DiscoveryPair, MockDiscovery, PEER_ENV, PEER_SECRET, discovery_pair, mock_discovery,
};
use endpoint::{EndpointFixture, assert_json};
use reinhardt::db::orm::Model;
use reinhardt::test::APIClient;
use rstest::rstest;
use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};
use tokio::time::timeout;

async fn discover(client: &APIClient, search: Value) -> Value {
	assert_json(
		client.post("/api/discover", &search, "json").await.unwrap(),
		200,
	)
}
async fn operator(app: &EndpointFixture, path: &str, body: Value) -> Value {
	assert_json(app.operator.post(path, &body, "json").await.unwrap(), 200)
}

#[rstest]
#[tokio::test]
async fn discovery_intersects_both_nodes_without_forwarding_subject_tokens(
	#[future] discovery_pair: DiscoveryPair,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	// Arrange
	let mut pair = Box::pin(discovery_pair).await;
	let a = &pair.source.app;
	let b = &pair.destination.app;
	let subject = &pair.source.subject;
	let mut policy = pair.source.policy;
	// Act
	let result = discover(subject, json!({})).await;
	// Assert
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(
		result["errors"][0]["error"],
		"authorized peer discovery unavailable"
	);
	let issued = operator(
		b,
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	operator(
		b,
		"/api/authorization/acme/peer-mappings",
		json!({
			"source_node":a.runtime.config.node_id,"source_tenant":"acme","source_subject":"alice",
			"credential_id":issued["credential"]["id"],"expected_revision":0,"enabled":true,
		}),
	)
	.await;
	let result = discover(subject, json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 2);
	assert_eq!(result["errors"], json!([]));
	// Identical local IDs must still be authorized within the remote namespace.
	policy["policies"].as_array_mut().unwrap().push(json!({
        "id":"hide-remote","effect":"deny","subjects":{"any":true},"actions":["registry.read"],
        "resources":{"kinds":["agent"],"ids":[qualified_agent(&b.runtime.config.node_id,"research","1.0.0")]},
    }));
	operator(
		a,
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	let result = discover(subject, json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(result["agents"][0]["node_id"], a.runtime.config.node_id);
	let mut observed = Vec::new();
	while let Ok(request) = pair.observed.try_recv() {
		observed.push(request);
	}
	assert_eq!(observed.len(), 3);
	for request in observed {
		assert_eq!(request.authorization, format!("Bearer {PEER_SECRET}"));
		assert_ne!(
			request.authorization,
			format!("Bearer {}", pair.source.subject_token)
		);
		assert_eq!(request.source, a.runtime.config.node_id);
		assert_eq!(request.protocol, "0.1");
		let mut keys: Vec<_> = request
			.body
			.as_object()
			.unwrap()
			.keys()
			.map(String::as_str)
			.collect();
		keys.sort_unstable();
		assert_eq!(keys, ["search", "subject", "tenant"]);
		assert_eq!(request.body["tenant"], "acme");
		assert_eq!(request.body["subject"], "alice");
		assert_eq!(request.body["search"]["kind"], "agent");
	}
	policy["policies"].as_array_mut().unwrap().pop();
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"no-disclosure","effect":"deny","subjects":{"any":true},
		"actions":["federation.discover"],"resources":{"kinds":["node"]},
	}));
	operator(
		a,
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy}),
	)
	.await;
	let result = discover(subject, json!({})).await;
	assert_eq!(result["errors"], json!([]));
	assert!(
		pair.observed.try_recv().is_err(),
		"denied discovery must not disclose tenant, subject or query"
	);
	policy["policies"].as_array_mut().unwrap().pop();
	operator(
		a,
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":policy}),
	)
	.await;
	operator(
		b,
		"/api/authorization/acme/catalog",
		json!({
			"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false,
		}),
	)
	.await;
	let result = discover(subject, json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(result["errors"], json!([]));
	operator(
		b,
		&format!(
			"/api/authorization/acme/credentials/{}/revoke",
			issued["credential"]["id"].as_str().unwrap()
		),
		json!({}),
	)
	.await;
	let result = discover(subject, json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(result["errors"].as_array().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn in_flight_discovery_retains_source_authority_and_subsequent_requests_honor_revocation(
	#[future] mock_discovery: MockDiscovery,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	// Arrange
	let fixture = Box::pin(mock_discovery).await;
	let app = &fixture.execution.app;
	fixture.pause.store(true, Ordering::SeqCst);
	let credential = AuthorizationCredential::objects()
		.filter(AuthorizationCredential::field_subject().eq("alice".to_owned()))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	let input = json!({});
	let active = fixture
		.execution
		.subject
		.post("/api/discover", &input, "json");
	tokio::pin!(active);
	// Act
	timeout(Duration::from_secs(5), async {
		tokio::select! {
			_ = &mut active => panic!("discovery completed before the peer barrier"),
			() = fixture.started.notified() => {},
		}
	})
	.await
	.expect("outbound discovery reaches the peer");
	let path = format!(
		"/api/authorization/acme/credentials/{}/revoke",
		credential.id
	);
	let revoker = app.operator.post(&path, &input, "json");
	tokio::pin!(revoker);
	// Assert
	assert!(
		timeout(Duration::from_millis(100), &mut revoker)
			.await
			.is_err(),
		"revocation must wait for the admitted boundary"
	);
	fixture.release.notify_one();
	let result = assert_json(
		timeout(Duration::from_secs(5), active)
			.await
			.unwrap()
			.unwrap(),
		200,
	);
	assert_eq!(result["agents"].as_array().unwrap().len(), 2);
	assert_json(
		timeout(Duration::from_secs(5), revoker)
			.await
			.unwrap()
			.unwrap(),
		200,
	);
	assert_json(
		fixture
			.execution
			.subject
			.post("/api/discover", &input, "json")
			.await
			.unwrap(),
		401,
	);
}

#[rstest]
#[tokio::test]
async fn source_filters_search_results_and_rejects_invalid_peer_metadata(
	#[future] mock_discovery: MockDiscovery,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	// Arrange
	let fixture = Box::pin(mock_discovery).await;
	let subject = &fixture.execution.subject;
	// Act
	let result = discover(subject, json!({"capability":"not-present"})).await;
	// Assert
	assert_eq!(result["agents"], json!([]));
	assert_eq!(result["errors"], json!([]));
	for selected in 1..=3 {
		fixture.mode.store(selected, Ordering::SeqCst);
		let result = discover(subject, json!({})).await;
		assert_eq!(result["agents"].as_array().unwrap().len(), 1);
		assert_eq!(
			result["agents"][0]["node_id"],
			fixture.execution.app.runtime.config.node_id
		);
		assert_eq!(
			result["errors"][0]["error"],
			"invalid peer discovery response"
		);
	}
}
