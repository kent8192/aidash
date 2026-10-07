//! Receiver-side execution inspection remains read-only under live policy changes.
#[path = "../../../execution/tests/support/endpoint.rs"]
mod endpoint;
#[path = "../../../execution/tests/support/execution.rs"]
mod execution_fixtures;
#[path = "../../../execution/tests/support/isolated.rs"]
mod isolated;
#[path = "../../../execution/tests/support/native_database.rs"]
mod native_database;
// This suite needs the successful provider fixture; adapter tests use the other modes.
#[allow(dead_code)]
#[path = "../../../execution/tests/provider_fixtures.rs"]
mod provider_fixtures;
use aidash_server::{
	apps::{execution::models::Run, federation::peer::models::Peer},
	domain::qualified_agent,
	registry::digest,
};
use endpoint::EndpointFixture;
use execution_fixtures::{ExecutionFixture, execution};
use reinhardt::db::orm::Model;
use reinhardt::test::fixtures::api_client_from_url;
use reinhardt::test::{APIClient, TestResponse};
use serde_json::{Value, json};

const PEER_ENV: &str = "AIDASH_SECRET_PEER_EXECUTION_FIXTURE";
const PEER_SECRET: &str = "peer-execution-fixture-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

fn decoded(response: TestResponse) -> (u16, Value) {
	assert_eq!(
		response.content_type(),
		Some("application/json"),
		"{}",
		response.text()
	);
	(response.status_code(), response.json_value().unwrap())
}
async fn operator(app: &EndpointFixture, path: &str, input: Value) -> (u16, Value) {
	decoded(app.operator.post(path, &input, "json").await.unwrap())
}
async fn peer_client(app: &EndpointFixture, token: &str) -> APIClient {
	let client = api_client_from_url(&app.server.url);
	client
		.set_header("Authorization", &format!("Bearer {token}"))
		.await
		.unwrap();
	client
		.set_header("x-aidash-node", "aidash://source")
		.await
		.unwrap();
	client.set_header("x-aidash-protocol", "0.2").await.unwrap();
	client
}
async fn inspect(client: &APIClient, input: Value) -> (u16, Value) {
	decoded(
		client
			.post("/federation/v0.1/scoped/execution/inspect", &input, "json")
			.await
			.unwrap(),
	)
}

#[rstest::rstest]
#[tokio::test]
async fn receiver_preflight_intersects_executor_and_mapping_without_admitting_a_run(
	#[future] execution: ExecutionFixture,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	let execution = Box::pin(execution).await;
	let app = &execution.app;
	let f = &app.runtime;
	let policy = execution.policy;
	let record = Peer::build()
		.node_id("aidash://source")
		.endpoint("http://127.0.0.1:1")
		.credential_env(PEER_ENV)
		.protocol_version("0.2")
		.enabled(true)
		.finish();
	Peer::objects()
		.create_with_conn(&mut app.database.lease.handle(), &record)
		.await
		.unwrap();
	let peer = peer_client(app, PEER_SECRET).await;
	let input = json!({"tenant":"remote","subject":"bob","agent":{"id":"research","version":"1.0.0"},"requirements":{}});
	assert_eq!(inspect(&peer, input.clone()).await.0, 403);
	let (_, issued) = operator(
		app,
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let credential = &issued["credential"]["id"];
	assert_eq!(operator(app,"/api/authorization/acme/peer-mappings",json!({"source_node":"aidash://source","source_tenant":"remote","source_subject":"bob","credential_id":credential,"enabled":true,"expected_revision":0})).await.0,200);
	for rejected in [
		issued["token"].as_str().unwrap(),
		f.config.api_token.as_str(),
	] {
		let client = peer_client(app, rejected).await;
		assert_eq!(inspect(&client, input.clone()).await.0, 401);
	}
	let (status, result) = inspect(&peer, input.clone()).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(result["node_id"], f.config.node_id);
	assert_eq!(result["agent"]["id"], "research");
	let definitions = result["definitions"].as_array().unwrap();
	assert_eq!(
		definitions.len(),
		3 + aidash_domain::registry::bindings::REQUIRED_TOOLS.len()
			+ aidash_domain::registry::bindings::DEFAULT_TOOLS.len()
	);
	for definition in definitions {
		let entry = f
			.registry
			.get(definition["entry"]["id"].as_str().unwrap(), "1.0.0")
			.await
			.unwrap();
		assert_eq!(
			definition["digest"],
			digest(&serde_json::to_value(entry).unwrap())
		);
		assert_eq!(definition["metadata"]["id"], definition["entry"]["id"]);
	}
	let mut mismatch = input.clone();
	mismatch["requirements"] = json!({"capability":"missing-capability"});
	assert_eq!(inspect(&peer, mismatch).await.0, 400);
	let executor = qualified_agent(&f.config.node_id, "research", "1.0.0");
	// Every denied action is tested for both the mapped root and the receiver's
	// executor. An allow on one must never override the other's explicit deny.
	let mut revision = 1;
	for subject in ["alice", executor.as_str()] {
		for (action, kind) in [
			("federation.execute", "node"),
			("agent.execute", "agent"),
			("registry.read", "agent"),
			("registry.read", "model"),
			("model.infer", "model"),
			("tool.invoke", "tool"),
		] {
			let mut denied = policy.clone();
			denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny","effect":"deny","subjects":{"ids":[subject]},"actions":[action],"resources":{"kinds":[kind]}}));
			assert_eq!(
				operator(
					app,
					"/api/authorization/acme",
					json!({"expected_revision":revision,"bundle":denied})
				)
				.await
				.0,
				200
			);
			revision += 1;
			assert_eq!(
				inspect(&peer, input.clone()).await.0,
				403,
				"{subject} {action} {kind}"
			);
		}
	}
	assert_eq!(
		operator(
			app,
			"/api/authorization/acme",
			json!({"expected_revision":revision,"bundle":policy})
		)
		.await
		.0,
		200
	);
	for id in ["model", "http", "research"] {
		assert_eq!(
			operator(
				app,
				"/api/authorization/acme/catalog",
				json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":1,"enabled":false})
			)
			.await
			.0,
			200
		);
		assert_eq!(inspect(&peer, input.clone()).await.0, 403);
		assert_eq!(
			operator(
				app,
				"/api/authorization/acme/catalog",
				json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":2,"enabled":true})
			)
			.await
			.0,
			200
		);
	}
	assert_eq!(inspect(&peer, input.clone()).await.0, 200);
	assert_eq!(
		operator(
			app,
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
	assert_eq!(inspect(&peer, input).await.0, 403);
	assert!(
		Run::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty(),
		"inspection must not create an executable run"
	);
}
