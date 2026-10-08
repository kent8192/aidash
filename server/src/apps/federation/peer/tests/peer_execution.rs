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
use reinhardt::test::TestResponse;
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

async fn inspect(app: &EndpointFixture, token: &str, input: Value) -> (u16, Value) {
	let authorization = format!("Bearer {token}");
	decoded(
		app.anonymous
			.post_raw_with_headers(
				"/federation/v0.1/scoped/execution/inspect",
				input.to_string().as_bytes(),
				"application/json",
				&[
					("Authorization", authorization.as_str()),
					("x-aidash-node", "aidash://source"),
					("x-aidash-protocol", "0.2"),
				],
			)
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
	let input = json!({"tenant":"remote","subject":"bob","agent":{"id":"research","version":"1.0.0"},"requirements":{}});
	assert_eq!(inspect(app, PEER_SECRET, input.clone()).await.0, 403);
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
		assert_eq!(inspect(app, rejected, input.clone()).await.0, 401);
	}
	let (status, result) = inspect(app, PEER_SECRET, input.clone()).await;
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
	assert_eq!(inspect(app, PEER_SECRET, mismatch).await.0, 400);
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
				inspect(app, PEER_SECRET, input.clone()).await.0,
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
		assert_eq!(inspect(app, PEER_SECRET, input.clone()).await.0, 403);
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
	assert_eq!(inspect(app, PEER_SECRET, input.clone()).await.0, 200);
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
	assert_eq!(inspect(app, PEER_SECRET, input).await.0, 403);
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

#[rstest::rstest]
#[tokio::test]
async fn public_agent_closures_are_authenticated_and_offers_pin_exact_receiver_definitions(
	#[from(endpoint::endpoint)] endpoint: endpoint::EndpointFuture,
	#[from(closure_peer_client)]
	#[with(endpoint.clone())]
	peer_client: futures_util::future::BoxFuture<'static, reinhardt::test::APIClient>,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	let app = endpoint.await;
	endpoint::register_fixture_agent(&app, "public-child").await;
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
	let client = peer_client.await;
	let path = "/federation/v0.1/discover/public-child/1.0.0/bindings";
	assert_eq!(app.anonymous.get(path).await.unwrap().status_code(), 400);
	assert_eq!(app.operator.get(path).await.unwrap().status_code(), 400);
	let (status, body) = decoded(client.get(path).await.unwrap());
	assert_eq!(status, 200, "{body}");
	let pinned: aidash_domain::registry::bindings::ForeignAgentSnapshot =
		serde_json::from_value(body).unwrap();
	pinned.validate().unwrap();
	assert_eq!(pinned.agent.registry_node, app.runtime.config.node_id);
	assert!(
		pinned
			.bindings
			.iter()
			.any(|binding| binding.alias.as_deref() == Some("task_delegate")
				&& binding.excluded_reason.is_some())
	);
	let workspace = endpoint::workspace(&app.operator, "pinned public child").await;
	let (status, task) = operator(
		&app,
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"pinned offer","description":"Execute the pinned public child","requirements":{},"dependencies":[],"parent_id":null}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let offer = json!({"task":task,"agent":pinned.agent.local(),"binding_snapshot":pinned});
	let mut changed = pinned.clone();
	let root = changed
		.definitions
		.iter_mut()
		.find(|definition| definition.identity == changed.agent)
		.unwrap();
	root.definition
		.name
		.insert("en".into(), "different public definition".into());
	*root = aidash_domain::registry::bindings::ResolvedDefinition::new(
		root.identity.clone(),
		root.definition.clone(),
	)
	.unwrap();
	changed.validate().unwrap();
	let mut mismatch = offer.clone();
	mismatch["binding_snapshot"] = serde_json::to_value(changed).unwrap();
	assert_eq!(
		decoded(
			client
				.post("/federation/v0.1/offers", &mismatch, "json")
				.await
				.unwrap()
		)
		.0,
		409
	);
	assert_eq!(
		Run::objects()
			.count_with_conn(&mut app.database.lease.handle())
			.await
			.unwrap(),
		0
	);
	let first = decoded(
		client
			.post("/federation/v0.1/offers", &offer, "json")
			.await
			.unwrap(),
	);
	assert_eq!(first.0, 200, "{}", first.1);
	let admitted: aidash_domain::Run = serde_json::from_value(first.1.clone()).unwrap();
	assert_eq!(
		admitted.context.binding_snapshot.as_deref(),
		Some(&pinned.snapshot())
	);
	assert_eq!(
		decoded(
			client
				.post("/federation/v0.1/offers", &offer, "json")
				.await
				.unwrap()
		),
		first
	);
	assert_eq!(
		decoded(
			client
				.post("/federation/v0.1/offers", &mismatch, "json")
				.await
				.unwrap()
		)
		.0,
		409
	);
	assert_eq!(
		Run::objects()
			.count_with_conn(&mut app.database.lease.handle())
			.await
			.unwrap(),
		1
	);
	Peer::objects()
		.filter(Peer::field_node_id().eq("aidash://source"))
		.update_fields_with_conn(
			&mut app.database.lease.handle(),
			[(Peer::field_enabled(), false)],
		)
		.await
		.unwrap();
	assert_eq!(client.get(path).await.unwrap().status_code(), 401);
}

#[rstest::rstest]
#[tokio::test]
async fn operator_permission_inspection_resolves_foreign_agents_without_subject_imports(
	#[from(endpoint::endpoint)]
	#[with("aidash://inspection-home")]
	home: endpoint::EndpointFuture,
	#[from(endpoint::anonymous_client)]
	#[with(home.clone())]
	reader_client: endpoint::ClientFuture,
	#[future]
	#[from(endpoint::endpoint)]
	#[with("aidash://inspection-child")]
	child: EndpointFixture,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	let home = home.await;
	let child = child.await;
	endpoint::register_fixture_agent(&home, "parent").await;
	endpoint::register_fixture_agent(&child, "public-child").await;
	for (owner, peer) in [(&home, &child), (&child, &home)] {
		let record = Peer::build()
			.node_id(&peer.runtime.config.node_id)
			.endpoint(&peer.server.url)
			.credential_env(PEER_ENV)
			.protocol_version("0.2")
			.enabled(true)
			.finish();
		Peer::objects()
			.create_with_conn(&mut owner.database.lease.handle(), &record)
			.await
			.unwrap();
	}
	let tool = json!({"id":"foreign-tool","version":"1.0.0","kind":"tool","name":{"en":"Foreign child"},"description":{"en":"Pinned public child"},"config":{"registry_node":home.runtime.config.node_id,"provider":"integration.agent@1","operation":"invoke","default_alias":"foreign_child","tier":"integration","transport":{"transport":"agent","node_id":child.runtime.config.node_id,"agent":{"id":"public-child","version":"1.0.0"}}}});
	let registered = operator(&home, "/api/registry", tool).await;
	assert_eq!(registered.0, 200, "{}", registered.1);
	let mut parent = home.runtime.registry.get("parent", "1.0.0").await.unwrap();
	parent.version = "1.0.1".into();
	parent.binding_normalization = None;
	parent.config["bindings"] = json!([{"kind":"tool","target":{"registry_node":home.runtime.config.node_id,"id":"foreign-tool","version":"1.0.0"},"narrow":{}}]);
	home.runtime.registry.register(parent).await.unwrap();
	let subject = endpoint::subject(&home, "reader", reader_client.await).await;
	let path = "/api/workbench/versions/parent/1.0.1/permissions";
	let input = json!({"tenant":"endpoint","subject":"reader"});
	let report = operator(&home, path, input.clone()).await;
	assert_eq!(report.0, 200, "{}", report.1);
	assert!(
		report.1["rows"]
			.as_array()
			.unwrap()
			.iter()
			.any(|row| row["reference"]["id"] == "foreign-tool")
	);
	assert!(report.1["rows"].as_array().unwrap().iter().all(|row| {
		!row["reference"]["id"]
			.as_str()
			.unwrap()
			.starts_with("public-child")
	}));
	assert_eq!(
		decoded(subject.post(path, &input, "json").await.unwrap()).0,
		403
	);
}

#[rstest::fixture]
fn closure_peer_client(
	#[from(endpoint::endpoint)] endpoint: endpoint::EndpointFuture,
) -> futures_util::future::BoxFuture<'static, reinhardt::test::APIClient> {
	Box::pin(async move {
		let app = endpoint.await;
		let client = reinhardt::test::fixtures::api_client_from_url(&app.server.url);
		for (name, value) in [
			("authorization", format!("Bearer {PEER_SECRET}")),
			("x-aidash-node", "aidash://source".into()),
			("x-aidash-protocol", "0.2".into()),
		] {
			client.set_header(name, &value).await.unwrap();
		}
		client
	})
}
