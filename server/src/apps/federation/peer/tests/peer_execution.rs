//! Receiver-side execution inspection remains read-only under live policy changes.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../execution/tests/support/endpoint.rs"]
mod endpoint;
#[path = "../../../execution/tests/support/execution.rs"]
mod execution_fixtures;
#[path = "../../../execution/tests/support/native_database.rs"]
mod native_database;
#[path = "../../../execution/tests/support/federation.rs"]
mod peering;
// This suite needs the successful provider fixture; adapter tests use the other modes.
#[allow(dead_code)]
#[path = "../../../execution/tests/provider_fixtures.rs"]
mod provider_fixtures;
use aidash_server::{apps::execution::models::Run, registry::digest};
use endpoint::EndpointFixture;
use execution_fixtures::{ExecutionFixture, execution};
use peering::{Bearer, Remote};
use reinhardt::db::orm::Model;
use reinhardt::test::TestResponse;
use serde_json::{Value, json};

const SOURCE: &str = "aidash://source";

fn sender() -> String {
	aidash_domain::nodes::NodeName::derived("source").to_string()
}

fn decoded(response: TestResponse) -> (u16, Value) {
	assert_eq!(
		response.content_type(),
		Some("application/json"),
		"{}",
		response.text()
	);
	(response.status_code(), response.json_value().unwrap())
}
async fn deployment_administrator(app: &EndpointFixture, path: &str, input: Value) -> (u16, Value) {
	if path == "/api/registry" {
		let entry = app
			.runtime
			.registry
			.register(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				serde_json::from_value(input).unwrap(),
			)
			.await
			.unwrap();
		return (200, serde_json::to_value(entry).unwrap());
	}
	if let Some(tenant) = path
		.strip_prefix("/api/authorization/")
		.and_then(|rest| rest.strip_suffix("/catalog"))
	{
		let binding = aidash_server::authorization::Authorization {
			pool: app.runtime.store.pool.clone(),
		}
		.set_catalog(
			tenant,
			&aidash_domain::nodes::NodeName::derived(tenant),
			&serde_json::from_value(input["entry"].clone()).unwrap(),
			input["expected_revision"].as_i64().unwrap(),
			input["enabled"].as_bool().unwrap(),
			"deployment_administrator",
		)
		.await
		.unwrap();
		return (200, serde_json::to_value(binding).unwrap());
	}
	decoded(
		app.deployment_administrator
			.post(path, &input, "json")
			.await
			.unwrap(),
	)
}

async fn inspect(
	app: &EndpointFixture,
	bearer: Bearer<'_>,
	subject: &str,
	input: Value,
) -> (u16, Value) {
	let receiver = endpoint::default_node(&app.runtime.store, "acme").await;
	decoded(
		peering::post_as(
			&app.anonymous,
			&receiver.to_string(),
			bearer,
			subject,
			"/federation/v0.1/scoped/execution/inspect",
			&input,
		)
		.await,
	)
}

#[rstest::rstest]
#[case::label(Some(("/label", json!("Default"))))]
#[case::default(Some(("/default", json!(true))))]
#[case::denied(None)]
#[tokio::test]
async fn receiver_preflight_uses_the_selected_nodes_policy_resource(
	#[future] execution: ExecutionFixture,
	#[case] condition: Option<(&str, Value)>,
) {
	let execution = Box::pin(execution).await;
	let app = &execution.app;
	let remote = Remote::start(SOURCE).await;
	let f = &peering::enable(&app.runtime, &app.context);
	peering::permit(f, &remote.endpoint);
	let receiving = endpoint::default_node(&f.store, "acme").await.node;
	assert_eq!(
		decoded(
			app.deployment_administrator
				.post(
					&peering::registration_path("acme", receiving.as_str()),
					&remote.registration(&sender()),
					"json",
				)
				.await
				.unwrap()
		)
		.0,
		200,
	);
	let (_, issued) = deployment_administrator(
		app,
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(
		deployment_administrator(
			app,
			"/api/authorization/acme/peer-mappings",
			json!({
				"node":receiving,"source_node":remote.node(&sender()),"source_subject":"bob",
				"credential_id":issued["credential"]["id"],"enabled":true,"expected_revision":0,
			})
		)
		.await
		.0,
		200
	);
	let mut policy = execution.policy.clone();
	let expected = if let Some((path, value)) = condition {
		policy["policies"] = json!([
			{"id":"definitions","effect":"allow","subjects":{"any":true},
			 "actions":["registry.read","agent.execute","model.infer","tool.invoke","skill.use","cluster.execute","decision.invoke"],"resources":{"kinds":["*"]}},
			{"id":"receiver","effect":"allow","subjects":{"any":true},"actions":["federation.execute"],
			 "resources":{"kinds":["node"],"ids":[receiving]},
			 "condition":{"op":"eq","left":{"source":"resource","path":path},"right":{"source":"literal","value":value}}},
		]);
		200
	} else {
		policy["policies"].as_array_mut().unwrap().push(json!({
			"id":"deny-receiver","effect":"deny","subjects":{"any":true},"actions":["federation.execute"],
			"resources":{"kinds":["node"],"ids":[receiving]},
		}));
		403
	};
	assert_eq!(
		deployment_administrator(
			app,
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let (status, result) = inspect(
		app,
		Bearer::Signed(&remote, sender()),
		"bob",
		json!({"agent":{"id":"research","version":"1.0.0"},"requirements":{}}),
	)
	.await;
	assert_eq!(status, expected, "{result}");
	if expected == 200 {
		assert_eq!(
			result["node_id"],
			f.config.deployment.node(receiving).to_string()
		);
	}
}

#[rstest::rstest]
#[tokio::test]
async fn receiver_preflight_intersects_executor_and_mapping_without_admitting_a_run(
	#[future] execution: ExecutionFixture,
) {
	let execution = Box::pin(execution).await;
	let app = &execution.app;
	let policy = execution.policy;
	let remote = Remote::start(SOURCE).await;
	let f = &peering::enable(&app.runtime, &app.context);
	peering::permit(f, &remote.endpoint);
	let receiving = endpoint::default_node(&f.store, "acme").await.node;
	let registered = app
		.deployment_administrator
		.post(
			&peering::registration_path("acme", receiving.as_str()),
			&remote.registration(&sender()),
			"json",
		)
		.await
		.unwrap();
	assert_eq!(decoded(registered).0, 200);
	let signed = || Bearer::Signed(&remote, sender());
	let input = json!({"agent":{"id":"research","version":"1.0.0"},"requirements":{}});
	assert_eq!(inspect(app, signed(), "bob", input.clone()).await.0, 403);
	let (_, issued) = deployment_administrator(
		app,
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let credential = &issued["credential"]["id"];
	assert_eq!(deployment_administrator(app,"/api/authorization/acme/peer-mappings",json!({"node":receiving,"source_node":remote.node(&sender()),"source_subject":"bob","credential_id":credential,"enabled":true,"expected_revision":0})).await.0,200);
	for rejected in [
		issued["token"].as_str().unwrap(),
		f.config.api_token.as_str(),
	] {
		assert_eq!(
			inspect(app, Bearer::Raw(rejected), "bob", input.clone())
				.await
				.0,
			401
		);
	}
	let (status, result) = inspect(app, signed(), "bob", input.clone()).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		result["node_id"],
		f.config.deployment.node(receiving.clone()).to_string()
	);
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
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				definition["entry"]["id"].as_str().unwrap(),
				"1.0.0",
			)
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
	assert_eq!(inspect(app, signed(), "bob", mismatch).await.0, 400);
	let executor = aidash_domain::agent_subject(
		&f.config.deployment.node(receiving.clone()).to_string(),
		"research",
		"1.0.0",
	);
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
				deployment_administrator(
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
				inspect(app, signed(), "bob", input.clone()).await.0,
				403,
				"{subject} {action} {kind}"
			);
		}
	}
	assert_eq!(
		deployment_administrator(
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
			deployment_administrator(
				app,
				"/api/authorization/acme/catalog",
				json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":id,"version":"1.0.0"},"expected_revision":1,"enabled":false})
			)
			.await
			.0,
			200
		);
		assert_eq!(inspect(app, signed(), "bob", input.clone()).await.0, 403);
		assert_eq!(
			deployment_administrator(
				app,
				"/api/authorization/acme/catalog",
				json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":id,"version":"1.0.0"},"expected_revision":2,"enabled":true})
			)
			.await
			.0,
			200
		);
	}
	assert_eq!(inspect(app, signed(), "bob", input.clone()).await.0, 200);
	assert_eq!(
		deployment_administrator(
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
	assert_eq!(inspect(app, signed(), "bob", input).await.0, 403);
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
async fn an_agent_cannot_bind_a_foreign_agent_without_the_removed_closure_import(
	#[from(endpoint::endpoint)]
	#[with("aidash://inspection-home")]
	home: endpoint::EndpointFuture,
	#[future]
	#[from(endpoint::endpoint)]
	#[with("aidash://inspection-child")]
	child: EndpointFixture,
) {
	let home = home.await;
	let child = child.await;
	endpoint::register_fixture_agent(&home, "parent").await;
	endpoint::register_fixture_agent(&child, "public-child").await;
	let home_runtime = peering::enable(&home.runtime, &home.context);
	let child_runtime = peering::enable(&child.runtime, &child.context);
	for (owner, runtime, peer, peer_runtime) in [
		(&home, &home_runtime, &child, &child_runtime),
		(&child, &child_runtime, &home, &home_runtime),
	] {
		peering::permit(runtime, &peer.server.url);
		endpoint::tenant_scope(&runtime.store, "acme").await;
		let node = endpoint::default_node(&runtime.store, "acme").await;
		let remote = endpoint::default_node(&peer_runtime.store, "acme").await;
		let registered = owner
			.deployment_administrator
			.post(
				&peering::registration_path("acme", node.node.as_str()),
				&peering::registration(
					&remote.to_string(),
					Some((&peer.server.url, &peering::key_id(peer_runtime))),
				),
				"json",
			)
			.await
			.unwrap();
		assert_eq!(decoded(registered).0, 200);
	}
	let tool = json!({"id":"foreign-tool","version":"1.0.0","kind":"tool","name":{"en":"Foreign child"},"description":{"en":"Pinned public child"},"config":{"registry_deployment":home.runtime.config.deployment,"provider":"integration.agent@1","operation":"invoke","default_alias":"foreign_child","tier":"integration","transport":{"transport":"agent","node_id":child.runtime.config.deployment,"agent":{"id":"public-child","version":"1.0.0"}}}});
	let registered = deployment_administrator(&home, "/api/registry", tool).await;
	assert_eq!(registered.0, 200, "{}", registered.1);
	let acme = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut parent = home
		.runtime
		.registry
		.get(&acme, "parent", "1.0.0")
		.await
		.unwrap();
	parent.version = "1.0.1".into();
	parent.binding_normalization = None;
	parent.config["bindings"] = json!([{"kind":"tool","target":{"registry_deployment":home.runtime.config.deployment,"id":"foreign-tool","version":"1.0.0"},"narrow":{}}]);
	assert!(matches!(
		home.runtime.registry.register(&acme, parent).await,
		Err(aidash_server::Error::Forbidden)
	));
	assert!(
		home.runtime
			.registry
			.get(&acme, "parent", "1.0.1")
			.await
			.is_err()
	);
}
