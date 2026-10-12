//! Real HTTP coverage for peer mappings and discovery under live authority changes.
#[path = "../../../execution/tests/support/endpoint.rs"]
mod endpoint;
#[path = "../../../execution/tests/support/execution.rs"]
mod execution_fixtures;
#[path = "../../../execution/tests/support/isolated.rs"]
mod isolated;
#[path = "../../../execution/tests/support/native_database.rs"]
mod native_database;
// Discovery uses the successful provider fixture; adapter suites use the other modes.
#[allow(dead_code)]
#[path = "../../../execution/tests/provider_fixtures.rs"]
mod provider_fixtures;

use aidash_server::apps::{
	federation::peer::models::{AuthorizationPeerMappingHistory, Peer},
	identity::models::AuthorizationCredential,
};
use chrono::{Duration, Utc};
use endpoint::EndpointFixture;
use execution_fixtures::{ExecutionFixture, execution};
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::future::Future;
use uuid::Uuid;

const SOURCE: &str = "aidash://source";
const PEER_ENV: &str = "AIDASH_SECRET_PEER_DISCOVERY_FIXTURE";
const PEER_SECRET: &str = "peer-discovery-fixture-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

#[fixture]
fn discovery(#[future] execution: ExecutionFixture) -> impl Future<Output = ExecutionFixture> {
	let execution = Box::pin(execution);
	async move {
		let execution = execution.await;
		let peer = Peer::build()
			.node_id(SOURCE)
			.endpoint("http://127.0.0.1:1")
			.credential_env(PEER_ENV)
			.protocol_version("0.2")
			.enabled(true)
			.finish();
		Peer::objects()
			.create_with_conn(&mut execution.app.database.lease.handle(), &peer)
			.await
			.unwrap();
		execution
	}
}

async fn request(
	app: &EndpointFixture,
	token: &str,
	method: &str,
	path: &str,
	value: Value,
) -> (u16, Value) {
	let authorization = format!("Bearer {token}");
	let headers = [("Authorization", authorization.as_str())];
	let response = match method {
		"GET" => async {
			let client = &(app.anonymous);
			let mut request = client.request(http::Method::GET, path);
			for (name, value) in &headers {
				request = request.header(*name, *value);
			}
			request.send().await
		}
		.await
		.unwrap(),
		"POST" => async {
			let client = &(app.anonymous);
			let mut request = client
				.request(http::Method::POST, path)
				.body(bytes::Bytes::copy_from_slice(value.to_string().as_bytes()))
				.header(http::header::CONTENT_TYPE, "application/json");
			for (name, value) in &headers {
				request = request.header(*name, *value);
			}
			request.send().await
		}
		.await
		.unwrap(),
		_ => panic!("unsupported fixture request method: {method}"),
	};
	(response.status_code(), response.json_value().unwrap())
}

async fn discover(
	app: &EndpointFixture,
	node: &str,
	token: &str,
	tenant: &str,
	subject: &str,
) -> (u16, Value) {
	let authorization = format!("Bearer {token}");
	let response = async {
		let client = &(app.anonymous);
		let mut request = client
			.request(http::Method::POST, "/federation/v0.1/scoped/discover")
			.body(bytes::Bytes::copy_from_slice(
				json!({"tenant":tenant,"subject":subject,"search":{}})
					.to_string()
					.as_bytes(),
			))
			.header(http::header::CONTENT_TYPE, "application/json");
		for (name, value) in &[
			("Authorization", authorization.as_str()),
			("x-aidash-node", node),
			("x-aidash-protocol", "0.2"),
		] {
			request = request.header(*name, *value);
		}
		request.send().await
	}
	.await
	.unwrap();
	(response.status_code(), response.json_value().unwrap())
}

#[rstest]
#[tokio::test]
async fn inbound_discovery_requires_exact_mapping_and_current_local_authority(
	#[future] discovery: ExecutionFixture,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	let execution = Box::pin(discovery).await;
	let app = &execution.app;
	let f = &app.runtime;
	let mut policy = execution.policy;
	// A peer record is a fixture prerequisite, not a substitute for the inbound
	// middleware: every discovery below passes through real authentication.
	let peer_token = PEER_SECRET;
	assert_eq!(
		discover(app, SOURCE, peer_token, "remote", "bob").await.0,
		403
	);
	let (_, issued) = request(
		app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let subject_token = execution.subject_token.as_str();
	let credential = &issued["credential"]["id"];
	let mut mapping = json!({"source_node":SOURCE,"source_tenant":"remote","source_subject":"bob","credential_id":credential,"enabled":true,"expected_revision":0});
	let path = "/api/authorization/acme/peer-mappings";
	assert_eq!(
		request(app, subject_token, "POST", path, mapping.clone())
			.await
			.0,
		403
	);
	let (status, binding) = request(app, &f.config.api_token, "POST", path, mapping.clone()).await;
	assert_eq!(status, 200, "{binding}");
	assert_eq!(binding["revision"], 1);
	let (status, mappings) = request(
		app,
		&f.config.api_token,
		"GET",
		"/api/authorization/acme/peer-mappings?offset=0&limit=1",
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{mappings}");
	assert_eq!(mappings, json!([binding]));
	for path in [
		"/api/authorization/acme/peer-mappings?offset=1&limit=1",
		"/api/authorization/other/peer-mappings?limit=1",
	] {
		let (status, mappings) = request(app, &f.config.api_token, "GET", path, json!({})).await;
		assert_eq!(status, 200, "{mappings}");
		assert_eq!(mappings, json!([]));
	}
	assert_eq!(
		request(app, subject_token, "GET", path, json!({})).await.0,
		403
	);
	for path in [
		"/api/authorization/acme/peer-mappings?offset=-1",
		"/api/authorization/acme/peer-mappings?limit=0",
		"/api/authorization/acme/peer-mappings?limit=201",
		"/api/authorization/acme/peer-mapping-history?after=-1",
		"/api/authorization/acme/peer-mapping-history?limit=0",
	] {
		let (status, body) = request(app, &f.config.api_token, "GET", path, json!({})).await;
		assert_eq!(status, 400, "{path}: {body}");
	}
	assert_eq!(
		request(app, &f.config.api_token, "POST", path, mapping.clone())
			.await
			.0,
		409
	);
	for (node, token, tenant, subject, expected) in [
		(SOURCE, peer_token, "remote", "bob", 200),
		(SOURCE, peer_token, "other", "bob", 403),
		(SOURCE, peer_token, "remote", "alice", 403),
		("aidash://forged", peer_token, "remote", "bob", 401),
		(SOURCE, subject_token, "remote", "bob", 401),
		(SOURCE, f.config.api_token.as_str(), "remote", "bob", 401),
	] {
		let (status, body) = discover(app, node, token, tenant, subject).await;
		assert_eq!(status, expected, "{node} {tenant}/{subject}: {body}");
		if status == 200 {
			assert_eq!(body.as_array().unwrap().len(), 1);
			assert_eq!(body[0]["id"], "research");
		}
	}
	mapping["expected_revision"] = json!(1);
	mapping["enabled"] = json!(false);
	assert_eq!(
		request(app, &f.config.api_token, "POST", path, mapping.clone())
			.await
			.0,
		200
	);
	assert_eq!(
		discover(app, SOURCE, peer_token, "remote", "bob").await.0,
		403
	);
	mapping["expected_revision"] = json!(2);
	mapping["enabled"] = json!(true);
	assert_eq!(
		request(app, &f.config.api_token, "POST", path, mapping.clone())
			.await
			.0,
		200
	);
	// Tenant approval is independent of the mapped subject's wildcard policy.
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		discover(app, SOURCE, peer_token, "remote", "bob").await.1,
		json!([])
	);
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":2,"enabled":true})
		)
		.await
		.0,
		200
	);
	// An explicit deny on the mapped local identity filters the remote result.
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-agent","effect":"deny","subjects":{"any":true},"actions":["agent.execute"],"resources":{"kinds":["agent"]}}));
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let (status, entries) = discover(app, SOURCE, peer_token, "remote", "bob").await;
	assert_eq!(status, 200);
	assert_eq!(entries, json!([]));
	// Credential revocation invalidates even an enabled, previously used map.
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			&format!(
				"/api/authorization/acme/credentials/{}/revoke",
				credential.as_str().unwrap()
			),
			json!({})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		discover(app, SOURCE, peer_token, "remote", "bob").await.0,
		403
	);
	mapping["expected_revision"] = json!(3);
	assert_eq!(
		request(app, &f.config.api_token, "POST", path, mapping.clone())
			.await
			.0,
		403
	);
	mapping["enabled"] = json!(false);
	assert_eq!(
		request(app, &f.config.api_token, "POST", path, mapping)
			.await
			.0,
		200
	);
	let count = AuthorizationPeerMappingHistory::objects()
		.all()
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap()
		.len();
	assert_eq!(count, 4, "failed writes must not create history");
	let (status, first) = request(
		app,
		&f.config.api_token,
		"GET",
		"/api/authorization/acme/peer-mapping-history?limit=2",
		json!({}),
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(first.as_array().unwrap().len(), 2);
	assert_eq!(first[0]["revision"], 1);
	let (_, second) = request(
		app,
		&f.config.api_token,
		"GET",
		&format!(
			"/api/authorization/acme/peer-mapping-history?after={}&limit=2",
			first[1]["sequence"]
		),
		json!({}),
	)
	.await;
	assert_eq!(second.as_array().unwrap().len(), 2);
	assert_eq!(second[0]["revision"], 3);
	assert_eq!(
		request(
			app,
			subject_token,
			"GET",
			"/api/authorization/acme/peer-mapping-history",
			json!({})
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"GET",
			"/api/authorization/acme/peer-mapping-history?limit=201",
			json!({})
		)
		.await
		.0,
		400
	);
}

#[rstest]
#[tokio::test]
async fn mappings_cannot_cross_tenants_and_expiry_or_disabled_subject_denies_discovery(
	#[future] discovery: ExecutionFixture,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	let execution = Box::pin(discovery).await;
	let app = &execution.app;
	let f = &app.runtime;
	let mut policy = execution.policy;
	let peer_token = PEER_SECRET;
	let (_, issued) = request(
		app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let credential = issued["credential"]["id"].clone();
	let mapping = json!({"source_node":SOURCE,"source_tenant":"remote","source_subject":"bob","credential_id":credential,"enabled":true,"expected_revision":0});
	let mut other_policy = policy.clone();
	other_policy["tenant"] = json!("other");
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/other",
			json!({"expected_revision":0,"bundle":other_policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/other/peer-mappings",
			mapping.clone()
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/peer-mappings",
			mapping.clone()
		)
		.await
		.0,
		200
	);
	let (_, other_issued) = request(
		app,
		&f.config.api_token,
		"POST",
		"/api/authorization/other/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let mut other_mapping = mapping.clone();
	other_mapping["credential_id"] = other_issued["credential"]["id"].clone();
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/other/peer-mappings",
			other_mapping.clone()
		)
		.await
		.0,
		409
	);
	other_mapping["expected_revision"] = json!(1);
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/other/peer-mappings",
			other_mapping
		)
		.await
		.0,
		409
	);
	policy["subjects"]["alice"]["enabled"] = json!(false);
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		discover(app, SOURCE, peer_token, "remote", "bob").await.0,
		403
	);
	policy["subjects"]["alice"]["enabled"] = json!(true);
	policy["policies"][0]["condition"] = json!({"op":"eq","left":{"source":"environment","path":"/transport"},"right":{"source":"literal","value":"federation"}});
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		discover(app, SOURCE, peer_token, "remote", "bob").await.0,
		200
	);
	// Expiry is a durable database clock check, including for active mappings.
	let mut connection = app.database.lease.handle();
	let mut credential = AuthorizationCredential::objects()
		.filter(
			AuthorizationCredential::field_id()
				.eq(Uuid::parse_str(credential.as_str().unwrap()).unwrap()),
		)
		.get_with_db(&mut connection)
		.await
		.unwrap();
	credential.created_at = Utc::now() - Duration::hours(2);
	credential.expires_at = Utc::now() - Duration::hours(1);
	AuthorizationCredential::objects()
		.update_with_conn(&mut connection, &credential)
		.await
		.unwrap();
	assert_eq!(
		discover(app, SOURCE, peer_token, "remote", "bob").await.0,
		403
	);
}
