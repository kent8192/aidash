use common::upstream_fixtures as upstream;
use http::Method;
use reinhardt::ServerRouter as Router;
use rstest::fixture;
use upstream::handler;

#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::{
	domain::NewTask,
	registry::{Entry, Package},
};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

fn tool(id: &str) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"tool","name":{"en":id},"description":{"en":"review regression"},"config":{"transport":"http","endpoint":"http://127.0.0.1:9/original","credential_env":null,"replay":"read_only"}})).unwrap()
}

async fn register_keyed(
	app: &common::TestApplication,
	token: &str,
	entry: &Entry,
	key: &str,
) -> (u16, Value) {
	let response = app
		.raw_http
		.request(Method::POST, app.url("/api/registry"))
		.header("authorization", format!("Bearer {token}"))
		.header("content-type", "application/json")
		.header("idempotency-key", key)
		.body(serde_json::to_vec(entry).unwrap())
		.send()
		.await
		.unwrap();
	let status = response.status().as_u16();
	let body = response.bytes().await.unwrap();
	(status, serde_json::from_slice(&body).unwrap())
}

#[rstest::rstest]
#[tokio::test]
async fn registry_server_ids_survive_retries_and_concurrent_requests(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let seeded_definitions = f.registry.list(&Default::default()).await.unwrap().len();
	let mut entry = tool("");
	entry.name.insert("en".into(), "calm-otter".into());
	let key = Uuid::new_v4().to_string();
	let (first, concurrent) = tokio::join!(
		register_keyed(&app, &f.config.api_token, &entry, &key),
		register_keyed(&app, &f.config.api_token, &entry, &key),
	);
	assert_eq!(first.0, 200, "{first:?}");
	assert_eq!(first, concurrent);
	assert_eq!(
		Uuid::parse_str(first.1["id"].as_str().unwrap())
			.unwrap()
			.get_version_num(),
		7
	);
	// A new router models a retry after the committed response was lost.
	assert_eq!(
		register_keyed(
			&common::application(f.clone()).await,
			&f.config.api_token,
			&entry,
			&key
		)
		.await,
		first
	);
	let mut changed = entry.clone();
	changed
		.description
		.insert("en".into(), "different input".into());
	assert_eq!(
		register_keyed(&app, &f.config.api_token, &changed, &key)
			.await
			.0,
		409
	);
	assert_eq!(
		f.registry.list(&Default::default()).await.unwrap().len(),
		seeded_definitions + 1
	);
	assert_eq!(
		f.store
			.events(0, None, 100)
			.await
			.unwrap()
			.iter()
			.filter(|event| event.kind == "registry.registered")
			.count(),
		1
	);
	assert_eq!(
		register_keyed(&app, &f.config.api_token, &entry, "invalid")
			.await
			.0,
		400
	);
	let next = register_keyed(
		&app,
		&f.config.api_token,
		&entry,
		&Uuid::new_v4().to_string(),
	)
	.await;
	assert_eq!(next.0, 200);
	assert_ne!(next.1["id"], first.1["id"]);
	// Failed validation must roll back the request-key reservation as well.
	let failed_key = Uuid::new_v4().to_string();
	let mut invalid = entry.clone();
	invalid.version = "invalid".into();
	assert_eq!(
		register_keyed(&app, &f.config.api_token, &invalid, &failed_key)
			.await
			.0,
		400
	);
	assert_eq!(
		register_keyed(&app, &f.config.api_token, &entry, &failed_key)
			.await
			.0,
		200
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn registry_assigns_uuid_v7_to_blank_ids_and_preserves_explicit_ids(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let mut generated = Vec::new();
	for id in ["", "", "explicit-tool"] {
		let mut entry = tool(id);
		entry.name.insert("en".into(), "calm-otter".into());
		let (status, result) = request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/registry",
			json!(entry),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		let assigned = result["id"].as_str().unwrap();
		if id.is_empty() {
			assert_eq!(Uuid::parse_str(assigned).unwrap().get_version_num(), 7);
			generated.push(assigned.to_owned());
		} else {
			assert_eq!(assigned, id);
		}
		assert_eq!(
			f.registry.get(assigned, "1.0.0").await.unwrap().id,
			assigned
		);
	}
	assert_ne!(generated[0], generated[1]);
	cleanup(f, &url, &schema).await;
}
#[rstest::rstest]
#[tokio::test]
async fn installation_reconfiguration_keeps_manifest_and_events_idempotent(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let package = Package {
		entity: tool("installed"),
		author: "test".into(),
		permissions: vec![],
		dependencies: vec![],
	};
	let original = package.entity.clone();
	let (status, published) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/marketplace",
		json!(package),
	)
	.await;
	assert_eq!(status, 200, "{published}");
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/marketplace",
			json!(package)
		)
		.await
		.0,
		200
	);
	let path = "/api/marketplace/installed/1.0.0/install";
	for endpoint in [
		"http://127.0.0.1:9/first",
		"http://127.0.0.1:9/second",
		"http://127.0.0.1:9/second",
	] {
		let (status, body) = request(
			&app,
			&f.config.api_token,
			"POST",
			path,
			json!({"digest":published["digest"],"config":{"endpoint":endpoint}}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		assert_eq!(
			f.registry.get("installed", "1.0.0").await.unwrap().config["endpoint"],
			endpoint
		);
	}
	let (_, packages) = request(
		&app,
		&f.config.api_token,
		"GET",
		"/api/marketplace",
		Value::Null,
	)
	.await;
	assert_eq!(packages[0]["manifest"]["entity"], json!(original));
	let events = f.store.events(0, None, 1000).await.unwrap();
	assert_eq!(
		events
			.iter()
			.filter(|e| e.kind == "package.published")
			.count(),
		1
	);
	assert_eq!(
		events
			.iter()
			.filter(|e| e.kind == "package.installed")
			.count(),
		2
	);
	let mut invalid = json!(package);
	invalid["dependecies"] = json!([]);
	// Preserve the previous backend's standard JSON extraction rejection.
	for (endpoint, input) in [
		("/api/marketplace", invalid),
		(
			path,
			json!({"digest":published["digest"],"configuration":{}}),
		),
	] {
		let rejected = app
			.raw_http
			.post(app.url(endpoint))
			.bearer_auth(&f.config.api_token)
			.json(&input)
			.send()
			.await
			.unwrap();
		assert_eq!(rejected.status().as_u16(), 422);
		assert_eq!(
			rejected.headers()["content-type"],
			"text/plain; charset=utf-8"
		);
		assert!(
			rejected
				.text()
				.await
				.unwrap()
				.contains("Failed to deserialize the JSON body")
		);
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn ancestor_dependencies_and_invalid_local_executor_are_rejected(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let workspace = f.store.create_workspace("tree", "goal").await.unwrap();
	let input = NewTask {
		title: "task".into(),
		description: "work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let parent = f
		.store
		.create_task(workspace.id, &input, "operator", None)
		.await
		.unwrap();
	let mut child = input.clone();
	child.parent_id = Some(parent.id);
	child.dependencies = vec![parent.id];
	assert!(
		f.store
			.create_task(workspace.id, &child, "operator", None)
			.await
			.is_err()
	);
	child.dependencies.clear();
	let child = f
		.store
		.create_task(workspace.id, &child, "operator", None)
		.await
		.unwrap();
	let mut grandchild = input;
	grandchild.parent_id = Some(child.id);
	grandchild.dependencies = vec![parent.id];
	assert!(
		f.store
			.create_task(workspace.id, &grandchild, "operator", None)
			.await
			.is_err()
	);
	let app = application_fixture.application.clone();
	let mut entry = tool("bad-executor");
	for agent in [
		json!({"id":"","version":"1.0.0"}),
		json!({"id":"bad/path","version":"1.0.0"}),
		json!({"id":"missing","version":"latest"}),
		json!({"id":"missing","version":"1.0.0"}),
	] {
		entry.config = json!({"transport":"agent","node_id":f.config.node_id,"agent":agent});
		assert_ne!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				"/api/registry",
				json!(entry)
			)
			.await
			.0,
			200
		);
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn visible_messages_and_events_survive_a_denied_burst(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let (mut policy, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	f.store
		.message(
			workspace,
			"allowed",
			"older-visible",
			Some("visible-message-old"),
		)
		.await
		.unwrap();
	// Public tool descriptions may mention privacy; detect the denied body itself.
	let denied_content = format!("denied-message-private-{}", Uuid::new_v4());
	for index in 0..110 {
		f.store
			.message(
				workspace,
				"denied",
				&denied_content,
				Some(&format!("denied-message-{index}")),
			)
			.await
			.unwrap();
	}
	let denied: Vec<String> = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("id::text"))
			.from(reinhardt::query::Alias::new("messages"))
			.and_where(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("sender")).eq("denied"),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_all(f.store.pool.driver())
	.await
	.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-messages","effect":"deny","subjects":{"ids":["alice"]},"actions":["message.read"],"resources":{"kinds":["message"],"ids":denied}}));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	for path in [format!("/api/workspaces/{workspace}"), "/api/state".into()] {
		let (status, body) = request(&app, &token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{body}");
		assert!(
			body["events"].to_string().contains("older-visible"),
			"{body}"
		);
		if path.contains("workspaces") {
			assert!(body["messages"].to_string().contains("older-visible"));
		}
		assert!(!body.to_string().contains(&denied_content), "{body}");
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn delegation_retry_and_run_message_have_one_durable_effect(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.create_workspace("operator", "goal").await.unwrap();
	let task = f
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "work".into(),
				description: "work".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let path = format!("/api/tasks/{}/delegate", task.id);
	for _ in 0..2 {
		let (status, body) = request(
			&app,
			&f.config.api_token,
			"POST",
			&path,
			json!({"node_id":f.config.node_id,"agent":{"id":"research","version":"1.0.0"}}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		assert_eq!(body["delivered"], true);
	}
	assert_eq!(
		f.store
			.events(0, Some(workspace.id), 1000)
			.await
			.unwrap()
			.iter()
			.filter(|e| e.kind == "task.delegated")
			.count(),
		1
	);
	let current = f.store.task(task.id).await.unwrap();
	let agent = f.registry.get("research", "1.0.0").await.unwrap();
	assert!(
		f.store
			.claim(
				task.id,
				current.revision,
				"aidash://other/agents/research@1.0.0",
				&agent
			)
			.await
			.is_err()
	);
	assert_eq!(
		f.store
			.claim(
				task.id,
				current.revision,
				&aidash_server::domain::qualified_agent(&f.config.node_id, "research", "1.0.0"),
				&agent
			)
			.await
			.unwrap()
			.status
			.as_str(),
		"CLAIMED"
	);
	let run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|r| r.task_id == task.id)
		.unwrap();
	let key = Uuid::new_v4();
	for _ in 0..2 {
		assert_eq!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				&format!("/api/runs/{}/message", run.id),
				json!({"content":"once","idempotency_key":key})
			)
			.await
			.0,
			200
		);
	}
	assert_eq!(
		f.store
			.snapshot(workspace.id)
			.await
			.unwrap()
			.messages
			.iter()
			.filter(|m| m.content == "once")
			.count(),
		1
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
fn remote_manifest_validates_secret_reference_without_resolving_it() {
	let mut entry = tool("remote-secret");
	entry.config["credential_env"] = json!("AIDASH_SECRET_REVIEW_REMOTE_ONLY_NOT_SET");
	let manifest: aidash_server::transactions::Manifest = serde_json::from_value(json!({
        "id":Uuid::new_v4(),"coordinator":"aidash://a","isolation":"serializable",
        "deadline":chrono::Utc::now()+chrono::Duration::minutes(1),
        "participants":[{"node_id":"aidash://a","mutations":[]},{"node_id":"aidash://b","mutations":[{"kind":"registry_register","entry":entry}]}]
    })).unwrap();
	aidash_server::transactions::validate(&manifest).unwrap();
	assert!(
		aidash_server::registry::validate(&entry).is_err(),
		"the assigned node still resolves its local credential"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_run_details_keep_memory_home_namespace(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let (_, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let run = f.store.runs().await.unwrap().remove(0);
	let (status, details) = request(
		&app,
		&token,
		"GET",
		&format!("/api/runs/{}", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{details}");
	assert!(
		details["memory"].is_null(),
		"an Agent without a memory provider has no bank binding"
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
fn agent_versions_fit_the_authorization_identity_limit() {
	let mut entry = tool(&"a".repeat(100));
	entry.kind = "agent".into();
	entry.config = json!({"model":{"id":"model","version":"1.0.0"},"instructions":"test"});
	entry.version = format!("1.0.0+{}", "x".repeat(33));
	aidash_server::registry::validate(&entry).unwrap();
	entry.version.push('x');
	assert!(aidash_server::registry::validate(&entry).is_err());
}

#[rstest::rstest]
#[tokio::test]
async fn mesh_rejects_a_peer_substituting_another_node_identity(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)]
	#[from(mesh_rejects_a_peer_substituting_another_node_identity_provider)]
	fixture: MeshRejectsAPeerSubstitutingAnotherNodeIdentityProvider,
) {
	let server = fixture.server;

	use reinhardt::query::{Alias, PostgresQueryBuilder, Query};
	let (f, url, schema) = application_fixture.runtime.parts();

	let endpoint = server.url.clone();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("peers"))
			.columns(
				[
					"node_id",
					"endpoint",
					"credential_env",
					"protocol_version",
					"enabled",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value("aidash://expected"),
				IntoValue::into_value(endpoint),
				IntoValue::into_value("AIDASH_SECRET_TEST_PEER"),
				IntoValue::into_value("0.1"),
				IntoValue::into_value(true),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	let (status, mesh) = request(
		&application_fixture.application,
		&f.config.api_token,
		"GET",
		"/api/mesh",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(mesh["nodes"], json!([]));
	assert_eq!(mesh["errors"][0]["node_id"], "aidash://expected");
	assert!(
		mesh["errors"][0]["error"]
			.as_str()
			.unwrap()
			.contains("identity mismatch")
	);
	drop(server);

	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn oversized_agent_instructions_skills_and_tools_are_rejected_at_registration(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let mut skill = tool("large-skill");
	skill.kind = "skill".into();
	skill.config = json!({"instructions":"x".repeat(60000)});
	f.registry.register(skill.clone()).await.unwrap();
	skill.id = "large-skill-second".into();
	f.registry.register(skill).await.unwrap();
	let mut large_tool = tool("large-schema");
	large_tool.schema = json!({"type":"object","description":"x".repeat(128000)});
	f.registry.register(large_tool).await.unwrap();
	for source in ["instructions", "skills", "tools"] {
		let mut agent = f.registry.get("research", "1.0.0").await.unwrap();
		agent.id = format!("oversized-{source}");
		agent.config[source] = match source {
			"instructions" => json!("x".repeat(128000)),
			"skills" => json!([
				{"id":"large-skill","version":"1.0.0"},
				{"id":"large-skill-second","version":"1.0.0"}
			]),
			_ => json!([{"id":"large-schema","version":"1.0.0"}]),
		};
		assert!(f.registry.register(agent.clone()).await.is_err());
		assert_eq!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				"/api/registry",
				json!(agent)
			)
			.await
			.0,
			400
		);
		assert!(f.registry.get(&agent.id, &agent.version).await.is_err());
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn malformed_broker_messages_do_not_stop_valid_delivery(
	#[from(common::native_application)] application_future: common::ApplicationFuture,
	#[future(awt)]
	#[from(review_bus)]
	#[with(application_future.clone())]
	bus: aidash_server::bus::EventBus,
) {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let application_fixture = application_future.await;
	let (f, url, schema) = application_fixture.runtime.parts();
	let id = Uuid::new_v4();
	for payload in [
		"not-json".to_owned(),
		"{}".to_owned(),
		"{\"id\":\"invalid\"}".to_owned(),
		json!({"id":id}).to_string(),
	] {
		bus.context
			.publish(bus.subject.clone(), payload.into())
			.await
			.unwrap()
			.await
			.unwrap();
	}
	// Act: start delivery after publishing the malformed broker inputs.
	let worker_bus = bus.clone();
	let worker_f = f.clone();
	let consumer = tokio::spawn(async move { worker_bus.consumer(worker_f).await });
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			let count: i64 = {
				let query_bind_1 = id;
				sqlx::query_scalar(
					&Query::select()
						.expr(reinhardt::query::Func::count(
							Expr::col(reinhardt::query::ColumnRef::Asterisk).into(),
						))
						.from(Alias::new("inbox"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("event_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(f.store.pool.driver())
				.await
			}
			.unwrap();
			if count == 1 {
				break;
			}
			assert!(
				!consumer.is_finished(),
				"consumer exited on malformed input"
			);
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
	})
	.await
	.unwrap();
	assert!(!consumer.is_finished());
	consumer.abort();
	let _ = consumer.await;
	bus.context.delete_stream(&bus.stream_name).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn registry_replays_emit_once_and_disabled_peers_can_lose_trust(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	for _ in 0..2 {
		let (status, body) = request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/registry",
			json!(tool("replayed")),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	assert_eq!(
		f.store
			.events(0, None, 1000)
			.await
			.unwrap()
			.iter()
			.filter(|e| e.kind == "registry.registered")
			.count(),
		1
	);
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("peers"))
			.columns(
				[
					"node_id",
					"endpoint",
					"credential_env",
					"protocol_version",
					"enabled",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value("aidash://disabled"),
				IntoValue::into_value("http://127.0.0.1:9"),
				IntoValue::into_value("AIDASH_SECRET_TEST_PEER"),
				IntoValue::into_value("0.1"),
				IntoValue::into_value(false),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("atomic_peer_trust"))
			.columns(["node_id", "enabled"].map(Alias::new))
			.values_panic([
				IntoValue::into_value("aidash://disabled"),
				IntoValue::into_value(true),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.control_pool.driver())
	.await
	.unwrap();
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/transactions/trust",
		json!({"node_id":"aidash://disabled", "enabled":false}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let enabled: bool = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("enabled"))
			.from(Alias::new("atomic_peer_trust"))
			.and_where(Expr::col(Alias::new("node_id")).eq("aidash://disabled"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.control_pool.driver())
	.await
	.unwrap();
	assert!(!enabled);
	assert_ne!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/transactions/trust",
			json!({"node_id":"aidash://disabled", "enabled":true})
		)
		.await
		.0,
		200
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn workspace_messages_deduplicate_retries_and_isolate_actor_and_workspace_keys(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let (_, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let key = Uuid::new_v4();
	let path = format!("/api/workspaces/{workspace}/messages");
	for actor in [&token, &f.config.api_token] {
		for _ in 0..2 {
			let (status, body) = request(
				&app,
				actor,
				"POST",
				&path,
				json!({"content":"once per actor","idempotency_key":key}),
			)
			.await;
			assert_eq!(status, 200, "{body}");
		}
		assert_eq!(
			request(
				&app,
				actor,
				"POST",
				&path,
				json!({"content":"changed","idempotency_key":key})
			)
			.await
			.0,
			409
		);
	}
	let snapshot = f.store.snapshot(workspace).await.unwrap();
	assert_eq!(
		snapshot
			.messages
			.iter()
			.filter(|m| m.content == "once per actor")
			.count(),
		2
	);
	assert_eq!(
		f.store
			.events(0, Some(workspace), 500)
			.await
			.unwrap()
			.iter()
			.filter(|e| e.kind == "message.created")
			.count(),
		2
	);
	let other = f.store.create_workspace("other", "goal").await.unwrap();
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			&format!("/api/workspaces/{}/messages", other.id),
			json!({"content":"another workspace","idempotency_key":key})
		)
		.await
		.0,
		200
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn operator_conversation_returns_the_committed_task_revision(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, response) = request(&app, &f.config.api_token, "POST", "/api/conversations", json!({"title":"Conversation","goal":"Work","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"})).await;
	assert_eq!(status, 200, "{response}");
	let task = f
		.store
		.task(response["task"]["id"].as_str().unwrap().parse().unwrap())
		.await
		.unwrap();
	assert!(task.revision > 0);
	assert_eq!(response["task"]["revision"], task.revision);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn plugin_control_shaped_data_does_not_suspend_execution(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)]
	#[from(plugin_control_shaped_data_does_not_suspend_execution_provider)]
	fixture: PluginControlShapedDataDoesNotSuspendExecutionProvider,
) {
	let output = fixture.state.output;
	let server = fixture.server;

	use aidash_server::harness::Harness;

	let (f, url, schema) = application_fixture.runtime.parts();

	let endpoint = server.url.clone();
	let app = application_fixture.application.clone();
	let (_, token, task) = bootstrap(&f, &app, &endpoint).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let harness = Harness {
		federation: f.clone(),
	};
	for _ in 0..12 {
		if !harness.worker_once().await.unwrap() {
			break;
		}
	}
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(
		run.phase().as_str(),
		"COMPLETED",
		"error={:?}, pending={}",
		run.error,
		json!(run.state)["data"]
	);
	assert!(
		json!(run.context)["history"]
			.as_array()
			.unwrap()
			.iter()
			.any(|e| e["kind"] == "tool" && e["result"] == output)
	);
	drop(server);

	cleanup(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder;

use reinhardt::query::ExprTrait;

use reinhardt::query::IntoValue;

use reinhardt::query::SimpleExpr;

#[fixture]
fn mesh_rejects_a_peer_substituting_another_node_identity_router() -> std::sync::Arc<Router> {
	std::sync::Arc::new(Router::new().handler(
		"/federation/v0.1/observe",
		handler(http::Method::GET, |_request: reinhardt::Request| async {
			reinhardt::Response::ok()
				.with_json(
					&json!({"node_id":"aidash://substituted","runs":[],"human_requests":[],"invocations":[]}),
				)
				.unwrap()
		}),
	))
}
struct MeshRejectsAPeerSubstitutingAnotherNodeIdentityProvider {
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn mesh_rejects_a_peer_substituting_another_node_identity_provider(
	#[from(mesh_rejects_a_peer_substituting_another_node_identity_router)] _router: std::sync::Arc<
		Router,
	>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> MeshRejectsAPeerSubstitutingAnotherNodeIdentityProvider {
	MeshRejectsAPeerSubstitutingAnotherNodeIdentityProvider { server }
}

#[derive(Clone)]
struct PluginControlShapedDataDoesNotSuspendExecutionState {
	output: Value,
}
#[fixture]
fn plugin_control_shaped_data_does_not_suspend_execution_state()
-> PluginControlShapedDataDoesNotSuspendExecutionState {
	PluginControlShapedDataDoesNotSuspendExecutionState {
		output: json!({"human_request_id":Uuid::new_v4(),"wait_seconds":60}),
	}
}
#[fixture]
fn plugin_control_shaped_data_does_not_suspend_execution_router(
	#[from(plugin_control_shaped_data_does_not_suspend_execution_state)]
	state: PluginControlShapedDataDoesNotSuspendExecutionState,
) -> std::sync::Arc<Router> {
	let output = state.output.clone();
	let result = output.clone();
	std::sync::Arc::new(Router::new()
        .handler("/effect", handler(http::Method::POST, move |_request: reinhardt::Request| { let result = result.clone(); async move { reinhardt::Response::ok().with_json(&result).unwrap() } }))
        .handler("/v1/chat/completions", handler(http::Method::POST, |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();async move {
            let context: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            let done = context["history"].as_array().unwrap().iter().any(|e| e["kind"] == "tool");
            let message = if done { json!({"role":"assistant","content":"Completed"}) } else {
                json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"plugin_0","arguments":"{}"}}]})
            };
            reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":if done {"stop"} else {"tool_calls"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
        }})))
}
struct PluginControlShapedDataDoesNotSuspendExecutionProvider {
	state: PluginControlShapedDataDoesNotSuspendExecutionState,
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn plugin_control_shaped_data_does_not_suspend_execution_provider(
	#[from(plugin_control_shaped_data_does_not_suspend_execution_state)]
	state: PluginControlShapedDataDoesNotSuspendExecutionState,
	#[from(plugin_control_shaped_data_does_not_suspend_execution_router)]
	#[with(state.clone())]
	_router: std::sync::Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> PluginControlShapedDataDoesNotSuspendExecutionProvider {
	PluginControlShapedDataDoesNotSuspendExecutionProvider { state, server }
}

#[fixture]
async fn review_bus(
	#[from(common::native_application)] application: common::ApplicationFuture,
) -> aidash_server::bus::EventBus {
	let runtime = application.await.runtime;
	aidash_server::bus::EventBus::connect(
		&runtime.federation.config.nats_url,
		&format!("aidash://review-{}", Uuid::new_v4().simple()),
	)
	.await
	.unwrap()
}
