use common::upstream_fixtures;
use http::Method;
use reinhardt::ServerRouter as Router;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use reinhardt::test::fixtures::http_client;
use reinhardt::test::fixtures::server::TestServerGuard;
use upstream_fixtures::{reply, upstream};
#[path = "support/legacy.rs"]
mod common;
use aidash_server::{
	config::Config,
	domain::*,
	federation::Federation,
	registry::{Entry, Package, Registry, Search},
	store::Store,
};
use common::{TestEnvironment, test_environment};
use http::StatusCode;
use serde_json::json;
use sqlx::ConnectOptions;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use uuid::Uuid;

use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use rstest::fixture;

type StoreFuture = Shared<BoxFuture<'static, StoreFixture>>;
#[derive(Clone)]
struct StoreFixture {
	store: Store,
	runtime: common::RuntimeFixture,
}
impl StoreFixture {
	fn parts(&self) -> (Store, String, String) {
		(
			self.store.clone(),
			self.runtime.url.clone(),
			self.runtime.schema.clone(),
		)
	}
}
#[fixture]
fn store(
	#[default("aidash://test")] node_id: &str,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> StoreFuture {
	let node_id = node_id.to_owned();
	async move {
		let mut runtime = runtime.await;
		runtime.federation.store.node_id = node_id.clone();
		Registry::new(runtime.federation.store.pool.clone(), &node_id)
			.unwrap()
			.seed_system()
			.await
			.unwrap();
		StoreFixture {
			store: runtime.federation.store.clone(),
			runtime,
		}
	}
	.boxed()
	.shared()
}

async fn cleanup(store: Store, url: &str, schema: &str) {
	store.control_pool.close().await;
	store.pool.close().await;
	common::cleanup_database(url, schema).await;
}
fn entry(kind: &str, id: &str, config: serde_json::Value) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id,"ja":"調査"},"description":{"en":"test fixture"},"capabilities":["web.search"],"languages":["en","ja"],"config":config})).unwrap()
}
async fn seed(registry: &Registry) -> Entry {
	registry.seed_system().await.unwrap();
	registry.register(entry("model","model",json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:9999/v1","credential_env":null,"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}))).await.unwrap();
	registry.register(entry("agent","research",json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Research","schema_version":1,"bindings":[],"remove_default":[]}))).await.unwrap()
}
fn new_task() -> NewTask {
	NewTask {
		title: "Research".into(),
		description: "Compare Rust frameworks".into(),
		requirements: json!({"capability":"web.search","language":"ja"}),
		dependencies: vec![],
		parent_id: None,
	}
}

#[rstest::rstest]
#[tokio::test]
async fn concurrent_claims_dependencies_and_idempotent_completion(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	let (store, url, schema) = _store_fixture.parts();
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	let agent = seed(&registry).await;
	let w = store
		.create_workspace("Research", "Compare frameworks")
		.await
		.unwrap();
	let task = store
		.create_task(w.id, &new_task(), "human", Some("create"))
		.await
		.unwrap();
	assert_eq!(
		task.id,
		store
			.create_task(w.id, &new_task(), "human", Some("create"))
			.await
			.unwrap()
			.id
	);
	let mut different = new_task();
	different.title = "Different".into();
	assert!(
		store
			.create_task(w.id, &different, "human", Some("create"))
			.await
			.is_err()
	);
	let owner = qualified_agent(&store.node_id, &agent.id, &agent.version);
	let (a, b) = tokio::join!(
		store.claim(task.id, 0, &owner, &agent),
		store.claim(task.id, 0, &owner, &agent)
	);
	assert_ne!(a.is_ok(), b.is_ok());
	let claimed = store.task(task.id).await.unwrap();
	assert_eq!(claimed.revision, 1);
	let mut dependent = new_task();
	dependent.dependencies = vec![task.id];
	// A prerequisite and its dependent are siblings, not a parent/child cycle.
	dependent.parent_id = None;
	let child = store
		.create_task(w.id, &dependent, "human", None)
		.await
		.unwrap();
	assert!(store.claim(child.id, 0, &owner, &agent).await.is_err());
	assert!(
		store
			.complete(
				task.id,
				&owner,
				"complete",
				&ArtifactInput {
					kind: "text".into(),
					name: "report".into(),
					content: json!("result")
				}
			)
			.await
			.is_err()
	);
	let running = store
		.transition(
			task.id,
			claimed.revision,
			&owner,
			aidash_server::domain::TaskStatus::Running,
		)
		.await
		.unwrap();
	assert_eq!(running.revision, 2);
	let artifact = ArtifactInput {
		kind: "text".into(),
		name: "report".into(),
		content: json!("result"),
	};
	let completed = store
		.complete(task.id, &owner, "complete", &artifact)
		.await
		.unwrap();
	let replay = store
		.complete(task.id, &owner, "complete", &artifact)
		.await
		.unwrap();
	assert_eq!(completed.revision, replay.revision);
	assert_eq!(store.snapshot(w.id).await.unwrap().artifacts.len(), 1);
	let mut bad = artifact.clone();
	bad.content = json!("different");
	assert!(
		store
			.complete(task.id, &owner, "complete", &bad)
			.await
			.is_err()
	);
	assert!(store.claim(child.id, 0, &owner, &agent).await.is_ok());
	let w2 = store.create_workspace("Other", "Other goal").await.unwrap();
	assert!(
		store
			.create_task(w2.id, &dependent, "human", None)
			.await
			.is_err()
	);
	let events = store.events(0, Some(w.id), 1000).await.unwrap();
	assert_eq!(
		events.iter().filter(|e| e.kind == "task.completed").count(),
		1
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn ordered_run_creation_requires_a_cache_salt_key_and_a_tenant(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange: a model that declares Ordered and an Agent that names it, on a
	// node without Cache Salt Keys.
	let (store, url, schema) = _store_fixture.parts();
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	registry.register(entry("model","model",json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:9999/v1","credential_env":null,"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{},"projection_versions":["legacy","ordered"]}))).await.unwrap();
	let agent = registry.register(entry("agent","research",json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Research","schema_version":1,"bindings":[],"remove_default":[],"projection_version":"ordered"}))).await.unwrap();
	let workspace = store
		.create_workspace("Research", "Compare frameworks")
		.await
		.unwrap();
	let task = store
		.create_task(workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	let owner = qualified_agent(&store.node_id, &agent.id, &agent.version);
	// Act: claiming would create the Run.
	let rejected = store.claim(task.id, 0, &owner, &agent).await;
	// Assert: no Run and no claim without a key.
	assert!(
		matches!(rejected, Err(aidash_server::Error::Invalid(_))),
		"{rejected:?}"
	);
	assert_eq!(store.task(task.id).await.unwrap().revision, 0);
	let salted = store.clone().with_cache_salt(Some(
		aidash_integrations::inference::CacheSaltKeys::new(
			&[aidash_integrations::inference::CacheSaltKey {
				version: 1,
				secret: "fixture-cache-salt-secret".into(),
			}],
			1,
		)
		.unwrap(),
	));
	// A key alone is not enough: this tenantless legacy workspace has no
	// Tenant to salt with, so the Run could never reach inference.
	let tenantless = salted.claim(task.id, 0, &owner, &agent).await;
	assert!(
		matches!(tenantless, Err(aidash_server::Error::Invalid(_))),
		"{tenantless:?}"
	);
	assert_eq!(store.task(task.id).await.unwrap().revision, 0);
	// Legacy Agents in the same workspace are unaffected.
	let legacy = registry.register(entry("agent","legacy-research",json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Research","schema_version":1,"bindings":[],"remove_default":[]}))).await.unwrap();
	let legacy_owner = qualified_agent(&store.node_id, &legacy.id, &legacy.version);
	let claimed = salted
		.claim(task.id, 0, &legacy_owner, &legacy)
		.await
		.unwrap();
	assert_eq!(claimed.revision, 1);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn lease_fencing_and_uncertain_effect_reconciliation(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	let (store, url, schema) = _store_fixture.parts();
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	let agent = seed(&registry).await;
	let w = store
		.create_workspace("Journal", "Recover safely")
		.await
		.unwrap();
	let task = store
		.create_task(w.id, &new_task(), "human", None)
		.await
		.unwrap();
	let run = store
		.accept_run(&task, &store.node_id, &agent.id, &agent.version)
		.await
		.unwrap();
	let token = Uuid::new_v4();
	let mut leased = store.lease_run(token, 30).await.unwrap().unwrap();
	assert_eq!(leased.id, run.id);
	leased.state = aidash_domain::run_state::RunState::Thinking(Default::default());
	leased.context.source_observation = Some(
		aidash_domain::context::sources::SourceObservation::new(
			"0:0:0".into(),
			"sha256:fixture".into(),
			json!({"memory":{"fact":"observed before inference"}}),
		)
		.unwrap(),
	);
	let revision = leased.revision;
	aidash_application::ports::execution::ExecutionStore::observe_sources(
		&store,
		&mut leased,
		token,
	)
	.await
	.unwrap();
	assert_eq!(leased.revision, revision + 1);
	assert_eq!(
		store
			.run(run.id)
			.await
			.unwrap()
			.context
			.source_observation
			.as_ref()
			.unwrap()
			.content,
		leased.context.source_observation.as_ref().unwrap().content
	);
	assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
	let first = store
		.invocation_start(
			&leased,
			token,
			"effect",
			"unsafe",
			&json!({"action":"write"}),
			false,
		)
		.await
		.unwrap();
	assert_eq!(first.status, "STARTED");
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("lease_until"),
					reinhardt::query::Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 SECOND'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	let new_token = Uuid::new_v4();
	let recovered = store.lease_run(new_token, 30).await.unwrap().unwrap();
	assert!(recovered.recovery.lease_recovered);
	assert_eq!(
		recovered
			.context
			.source_observation
			.as_ref()
			.unwrap()
			.content,
		leased.context.source_observation.as_ref().unwrap().content
	);
	assert!(
		aidash_application::ports::execution::ExecutionStore::observe_sources(
			&store,
			&mut leased,
			token
		)
		.await
		.is_err()
	);
	assert!(
		store
			.invocation_finish(&leased, token, "effect", &json!("stale"))
			.await
			.is_err()
	);
	let uncertain = store
		.invocation_start(
			&recovered,
			new_token,
			"effect",
			"unsafe",
			&json!({"action":"write"}),
			false,
		)
		.await
		.unwrap();
	assert_eq!(uncertain.status, "UNCERTAIN");
	let request = store
		.human_request(
			&recovered,
			"CONFIRMATION",
			"Verify the external result",
			"effect:reconcile",
		)
		.await
		.unwrap();
	store
		.answer(request.id, json!({"result":"verified"}))
		.await
		.unwrap();
	store
		.invocation_finish(&recovered, new_token, "effect", &json!("verified"))
		.await
		.unwrap();
	let replay = store
		.invocation_start(
			&recovered,
			new_token,
			"effect",
			"unsafe",
			&json!({"action":"write"}),
			false,
		)
		.await
		.unwrap();
	assert_eq!(replay.status.as_str(), "COMPLETED");
	assert_eq!(replay.result, Some(json!("verified")));
	assert!(
		store
			.invocation_start(
				&recovered,
				new_token,
				"effect",
				"unsafe",
				&json!({"action":"other"}),
				false
			)
			.await
			.is_err()
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn registry_installation_versions_and_authenticated_api(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(authenticated_runtime)]
	#[with(_store_fixture.clone())]
	_api_runtime: common::RuntimeFuture,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings::default(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), _api_runtime.clone())]
	_application: common::ApplicationFixture,
	http_client: reqwest::Client,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	seed(&registry).await;
	let skill = entry(
		"skill",
		"research-guide",
		json!({"instructions":"Keep citations"}),
	);
	let package = Package {
		entity: skill.clone(),
		author: "Fixture".into(),
		permissions: vec![],
		dependencies: vec![],
	};
	let record = registry.publish(package.clone()).await.unwrap();
	assert!(
		registry
			.install(&skill.id, &skill.version, "sha256:wrong", json!({}))
			.await
			.is_err()
	);
	registry
		.install(
			&skill.id,
			&skill.version,
			&record.digest,
			json!({"instructions":"Use node-local citations"}),
		)
		.await
		.unwrap();
	assert_eq!(
		registry.get(&skill.id, &skill.version).await.unwrap(),
		Entry {
			binding_normalization: None,
			config: json!({"instructions":"Use node-local citations"}),
			..skill.clone()
		}
	);
	let mut changed = package;
	changed
		.entity
		.description
		.insert("en".into(), "Changed".into());
	assert!(registry.publish(changed).await.is_err());
	let matches = registry
		.list(&Search {
			kind: Some("agent".into()),
			capability: Some("web.search".into()),
			language: Some("ja".into()),
			..Default::default()
		})
		.await
		.unwrap();
	assert_eq!(matches.len(), 1);
	let _f = _api_runtime.await.federation;

	let router = _application.application.clone();
	let unauth = http_client
		.request(Method::GET, router.url("/api/state"))
		.send()
		.await
		.unwrap();
	assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
	let auth = http_client
		.request(Method::GET, router.url("/api/state"))
		.header("Authorization", "Bearer test-access-token")
		.send()
		.await
		.unwrap();
	assert_eq!(auth.status(), StatusCode::OK);
	let peer = http_client
		.request(Method::POST, router.url("/federation/v0.1/discover"))
		.header("x-aidash-node", "aidash://intruder")
		.header("x-aidash-protocol", "0.2")
		.header("content-type", "application/json")
		.body("{}")
		.send()
		.await
		.unwrap();
	assert_eq!(peer.status(), StatusCode::UNAUTHORIZED);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn human_requests_controls_and_cancellation_before_dependencies_finish(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(authenticated_runtime)]
	#[with(_store_fixture.clone())]
	_api_runtime: common::RuntimeFuture,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings::default(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), _api_runtime.clone())]
	_application: common::ApplicationFixture,
	http_client: reqwest::Client,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	let agent = seed(&registry).await;
	let workspace = store
		.create_workspace("Controls", "Keep human decisions durable")
		.await
		.unwrap();
	let dependency = store
		.create_task(workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	let mut input = new_task();
	input.dependencies = vec![dependency.id];
	let task = store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let run = store
		.accept_run(&task, &store.node_id, &agent.id, &agent.version)
		.await
		.unwrap();
	let federation = _api_runtime.await.federation;
	let harness = aidash_server::harness::Harness {
		federation: federation.clone(),
	};
	let outage_pause = Query::update()
		.table(Alias::new("runs"))
		.value(Alias::new("control"), "PAUSED")
		.value(Alias::new("error"), "identity status unavailable")
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run.id)))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&outage_pause)
		.execute(store.pool.driver())
		.await
		.unwrap();
	let explicitly_paused = store
		.control(run.id, aidash_server::domain::RunControlAction::Pause)
		.await
		.unwrap();
	assert_eq!(explicitly_paused.control.as_str(), "PAUSED");
	assert_eq!(explicitly_paused.error, None);
	assert!(!harness.worker_once().await.unwrap());
	assert_eq!(
		store
			.control(run.id, aidash_server::domain::RunControlAction::Resume)
			.await
			.unwrap()
			.control
			.as_str(),
		"ACTIVE"
	);
	for kind in [
		"QUESTION",
		"APPROVAL_REQUIRED",
		"CONFIRMATION",
		"INFORMATION_REQUEST",
	] {
		let request = store
			.human_request(&run, kind, "Proceed?", kind)
			.await
			.unwrap();
		assert_eq!(
			store
				.human_request(&run, kind, "Proceed?", kind)
				.await
				.unwrap()
				.id,
			request.id
		);
		assert!(
			store
				.human_request(&run, kind, "Changed?", kind)
				.await
				.is_err()
		);
		assert!(store.answer(request.id, json!(null)).await.is_err());
		assert_eq!(
			store
				.answer(request.id, json!(false))
				.await
				.unwrap()
				.response,
			Some(json!(false))
		);
		assert_eq!(
			store.answer(request.id, json!(false)).await.unwrap().id,
			request.id
		);
		assert!(store.answer(request.id, json!(true)).await.is_err());
	}
	let router = _application.application.clone();
	let response = http_client
		.request(
			Method::POST,
			router.url(format!("/api/runs/{}/message", run.id)),
		)
		.header("Authorization", "Bearer test-access-token")
		.header("content-type", "application/json")
		.body(r#"{"content":"Keep the source citations."}"#)
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::OK);
	let snapshot = store.snapshot(workspace.id).await.unwrap();
	assert_eq!(snapshot.messages[0].sender, "human");
	store
		.control(run.id, aidash_server::domain::RunControlAction::Cancel)
		.await
		.unwrap();
	assert!(harness.worker_once().await.unwrap());
	assert_eq!(
		store.run(run.id).await.unwrap().phase().as_str(),
		"CANCELLED"
	);
	assert_eq!(
		store.task(task.id).await.unwrap().status.as_str(),
		"CANCELLED"
	);
	assert_eq!(
		store.task(dependency.id).await.unwrap().status.as_str(),
		"OPEN"
	);
	assert!(
		store
			.control(run.id, aidash_server::domain::RunControlAction::Resume)
			.await
			.is_err()
	);
	cleanup(store, &url, &schema).await;
}

#[fixture]
fn peer_client() -> reqwest::Client {
	// Outage/retry tests intentionally bound a failed peer request to two seconds.
	reqwest::Client::builder()
		.timeout(std::time::Duration::from_secs(2))
		.build()
		.unwrap()
}
#[fixture]
fn federation(
	#[from(store)] store_fixture: StoreFuture,
	peer_client: reqwest::Client,
) -> common::RuntimeFuture {
	async move {
		let store_fixture = store_fixture.await;
		let store = &store_fixture.store;
		let federation = Federation {
			sandbox: Default::default(),
			gcip: None,
			store: store.clone(),
			registry: Registry::new(store.pool.clone(), &store.node_id).unwrap(),
			config: Config {
				node_id: store.node_id.clone(),
				endpoint: "http://127.0.0.1:18080".into(),
				database_url: store.pool.connect_options().to_url_lossy().to_string(),
				nats_url: "nats://127.0.0.1:1".into(),
				api_token: "test-access-token".into(),
				web_dir: "web/dist".into(),
				lease_seconds: 30,
				default_host_packages: vec![],
				oidc: None,
				gcip: None,
			},
			client: peer_client,
			notify: Arc::new(tokio::sync::Notify::new()),
		};
		let mut runtime = store_fixture.runtime;
		runtime.federation = federation;
		runtime
	}
	.boxed()
	.shared()
}

async fn running_task(store: &Store, agent: &Entry, workspace: Uuid, parent: Option<Uuid>) -> Task {
	let mut input = new_task();
	input.parent_id = parent;
	let task = store
		.create_task(workspace, &input, "human", None)
		.await
		.unwrap();
	let owner = qualified_agent(&store.node_id, &agent.id, &agent.version);
	let task = store
		.claim(task.id, task.revision, &owner, agent)
		.await
		.unwrap();
	store
		.transition(
			task.id,
			task.revision,
			&owner,
			aidash_server::domain::TaskStatus::Running,
		)
		.await
		.unwrap()
}
async fn final_response(store: &Store, task: Uuid) {
	let response = aidash_server::provider::ModelResponse {
		text: "Report using the available results".into(),
		..Default::default()
	};
	{
		let query_bind_1 = task;
		let query_bind_2 = common::tool_pending(json!({"response":response,"cursor":0}));
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn parent_can_finish_after_explicit_child_abandonment(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings::default(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), _federation.clone())]
	_application: common::ApplicationFixture,
	http_client: reqwest::Client,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let workspace = store
		.create_workspace("Partial results", "Resolve terminal children")
		.await
		.unwrap();
	let parent = running_task(&store, &agent, workspace.id, None).await;
	let mut children = Vec::new();
	for status in [
		TaskStatus::Failed,
		TaskStatus::Blocked,
		TaskStatus::Cancelled,
	] {
		let child = running_task(&store, &agent, workspace.id, Some(parent.id)).await;
		let child = store
			.transition(
				child.id,
				child.revision,
				child.owner.as_deref().unwrap(),
				status,
			)
			.await
			.unwrap();
		{
			let query_bind_1 = child.id;
			sqlx::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value_expr(
						reinhardt::query::Alias::new("control"),
						reinhardt::query::Expr::cust("'PAUSED'"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(store.pool.driver())
			.await
		}
		.unwrap();
		children.push(child);
	}
	final_response(&store, parent.id).await;
	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	harness.worker_once().await.unwrap();
	let run: Run = {
		let query_bind_1 = parent.id;
		aidash_server::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(run.phase().as_str(), "WAITING");
	let request_id = request_id(&run);
	for child in children {
		assert!(
			store
				.abandon_task(child.id, child.revision + 1, "No longer needed")
				.await
				.is_err()
		);
		assert!(
			store
				.abandon_task(child.id, child.revision, " ")
				.await
				.is_err()
		);
		let app = _application.application.clone();
		let response = http_client
			.request(
				Method::POST,
				app.url(format!("/api/tasks/{}/abandon", child.id)),
			)
			.header("authorization", "Bearer test-access-token")
			.header("content-type", "application/json")
			.body(
				json!({"revision":child.revision,"reason":"Operator accepts partial results"})
					.to_string(),
			)
			.send()
			.await
			.unwrap();
		assert_eq!(response.status(), StatusCode::OK);
		assert_eq!(
			store.task(child.id).await.unwrap().status.as_str(),
			"ABANDONED"
		);
	}
	store
		.answer(request_id, json!("Continue with the remaining results"))
		.await
		.unwrap();
	harness.worker_once().await.unwrap();
	assert_eq!(
		store.run(run.id).await.unwrap().phase().as_str(),
		"THINKING"
	);
	final_response(&store, parent.id).await;
	harness.worker_once().await.unwrap();
	assert_eq!(
		store.task(parent.id).await.unwrap().status.as_str(),
		"COMPLETED"
	);
	assert_eq!(
		store.snapshot(workspace.id).await.unwrap().artifacts.len(),
		1
	);
	let events = store.events(0, Some(workspace.id), 1000).await.unwrap();
	assert_eq!(
		events
			.iter()
			.filter(|e| e.kind == "task.abandoned"
				&& e.data["reason"] == "Operator accepts partial results")
			.count(),
		3
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn successful_tool_retry_resets_the_next_invocation_budget(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
	#[from(successful_tool_retry_resets_the_next_invocation_budget_router)] _router: Arc<Router>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();

	let endpoint = server.url.clone();
	let f = _federation.await.federation;
	seed(&f.registry).await;
	f.registry
		.register(entry(
			"tool",
			"retry-tool",
			json!({"registry_node":f.config.node_id,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":endpoint,"credential_env":null,"replay":"idempotent"}}),
		))
		.await
		.unwrap();
	let agent = f.registry.register(entry("agent", "retry-agent", json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Test retries","schema_version":1,"bindings":[{"kind":"tool","target":{"registry_node":f.config.node_id,"id":"retry-tool","version":"1.0.0"},"alias":"plugin_0","narrow":{}}],"remove_default":[]}))).await.unwrap();
	let workspace = store
		.create_workspace("Retries", "Independent budgets")
		.await
		.unwrap();
	let task = running_task(&store, &agent, workspace.id, None).await;
	let response = aidash_server::provider::ModelResponse {
		tool_calls: (1..=2)
			.map(|call| aidash_server::provider::ToolCall {
				id: call.to_string(),
				name: "plugin_0".into(),
				arguments: json!({"call":call}),
			})
			.collect(),
		..Default::default()
	};
	{
		let query_bind_1 = task.id;
		let query_bind_2 = {
			let mut pending = common::tool_pending(json!({"response":response,"cursor":0}));
			pending["recovery"]["retry"] = json!(RetryState {
				count: 5,
				at: chrono::Utc::now() - chrono::Duration::seconds(1)
			});
			pending
		};
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(
					reinhardt::query::Alias::new("error"),
					reinhardt::query::Expr::cust("'prior transient error'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	let harness = aidash_server::harness::Harness { federation: f };
	harness.worker_once().await.unwrap();
	assert!(approve_fixture_call(&store).await);
	harness.worker_once().await.unwrap();
	harness.worker_once().await.unwrap();
	let run: Run = {
		let query_bind_1 = task.id;
		aidash_server::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(json!(run.state)["data"]["cursor"], 1);
	assert!(run.recovery.retry.is_none());
	assert!(run.error.is_none());
	harness.worker_once().await.unwrap();
	assert!(approve_fixture_call(&store).await);
	harness.worker_once().await.unwrap();
	harness.worker_once().await.unwrap();
	let run = store.run(run.id).await.unwrap();
	assert_eq!(run.phase().as_str(), "TOOL_CALL");
	assert_eq!(run.recovery.retry.as_ref().unwrap().count, 1);
	assert_eq!(
		store.task(task.id).await.unwrap().status.as_str(),
		"RUNNING"
	);
	drop(server);
	cleanup(store, &url, &schema).await;
}

async fn approve_fixture_call(store: &Store) -> bool {
	let run = store.runs().await.unwrap().remove(0);
	let RunState::Waiting(waiting) = &run.state else {
		return false;
	};
	let WaitingState::ExternalApproval { request_id, .. } = waiting.as_ref() else {
		return false;
	};
	let human = aidash_application::ports::execution::ExecutionStore::human_request_by_id(
		store,
		*request_id,
	)
	.await
	.unwrap();
	assert_eq!(human.kind, "APPROVAL_REQUIRED");
	assert!(
		human
			.prompt
			.starts_with("Approve this exact external tool action once?")
	);
	store
		.answer(*request_id, json!({"approved":true}))
		.await
		.unwrap();
	true
}

struct WriteApproval {
	store: Store,
	url: String,
	schema: String,
	harness: aidash_server::harness::Harness,
	run: Run,
	first: Uuid,
	effects: Arc<std::sync::atomic::AtomicUsize>,
	server: TestServerGuard,
	_runtime: common::RuntimeFixture,
}

#[rstest::fixture]
async fn write_approval(
	#[default(false)] expired: bool,
	#[default(false)] prior_response: bool,
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
	#[from(upstream_fixtures::hits)] effects: Arc<std::sync::atomic::AtomicUsize>,
	#[from(write_approval_router)]
	#[with(effects.clone())]
	_router: Arc<Router>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
) -> WriteApproval {
	let (store, url, schema) = _store_fixture.clone().await.parts();

	let endpoint = server.url.clone();
	let f = _federation.await.federation;
	seed(&f.registry).await;
	f.registry
		.register(entry(
			"tool",
			"managed-write",
			json!({"registry_node":f.config.node_id,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":endpoint,"credential_env":null,"replay":"unsafe"}}),
		))
		.await
		.unwrap();
	let agent = f.registry.register(entry("agent", "managed-agent", json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Use the selected tool","schema_version":1,"bindings":[{"kind":"tool","target":{"registry_node":f.config.node_id,"id":"managed-write","version":"1.0.0"},"alias":"plugin_0","narrow":{}}],"remove_default":[]}))).await.unwrap();
	let workspace = store
		.create_workspace("Approval", "Check external effects")
		.await
		.unwrap();
	let task = running_task(&store, &agent, workspace.id, None).await;
	let response = aidash_server::provider::ModelResponse {
		tool_calls: (1..=2)
			.map(|value| aidash_server::provider::ToolCall {
				id: value.to_string(),
				name: "plugin_0".into(),
				arguments: json!({"value":value}),
			})
			.collect(),
		..Default::default()
	};
	{
		let query_bind_1 = task.id;
		let query_bind_2 = common::tool_pending(json!({"response":response,"cursor":0}));
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::value("TOOL_CALL"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					Expr::value(query_bind_2.to_owned()),
				)
				.and_where(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("task_id"))
						.eq(Expr::value(query_bind_1.to_owned())),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	let harness = aidash_server::harness::Harness { federation: f };
	harness.worker_once().await.unwrap();
	let run: Run = {
		let query_bind_1 = task.id;
		aidash_server::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("*"))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("task_id"),
					))
					.eq(SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					)),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(store.pool.driver())
		.await
	}
	.unwrap();
	let first: Uuid = request_id(&run);
	if expired {
		use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
		let past = chrono::Utc::now() - chrono::Duration::minutes(16);
		{
			let query_bind_1 = first;
			let query_bind_2 = past;
			sqlx::query(
				&Query::update()
					.table(Alias::new("human_requests"))
					.value_expr(
						Alias::new("created_at"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(store.pool.driver())
			.await
		}
		.unwrap();
		let mut state = run.state.clone();
		let RunState::Waiting(waiting) = &mut state else {
			panic!("expected approval wait")
		};
		let WaitingState::ExternalApproval { expires_at, .. } = waiting.as_mut() else {
			panic!("expected external approval")
		};
		*expires_at = past;
		let pending = json!({"state_version":run.state_version,"data":json!(state)["data"],"recovery":run.recovery});
		{
			let query_bind_1 = run.id;
			let query_bind_2 = pending;
			sqlx::query(
				&Query::update()
					.table(Alias::new("runs"))
					.value_expr(
						Alias::new("pending"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(store.pool.driver())
			.await
		}
		.unwrap();
	}
	if prior_response {
		{
			let query_bind_1 = first;
			let query_bind_2 = json!({"approved":true});
			sqlx::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("human_requests"))
					.value_expr(
						reinhardt::query::Alias::new("response"),
						Expr::value(query_bind_2.to_owned()),
					)
					.value_expr(
						reinhardt::query::Alias::new("answered_by"),
						reinhardt::query::Expr::value("human"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".into(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(store.pool.driver())
			.await
		}
		.unwrap();
	}
	WriteApproval {
		store,
		url,
		schema,
		harness,
		run,
		first,
		effects,
		server,
		_runtime: _store_fixture.clone().await.runtime,
	}
}

#[rstest::rstest]
#[case(false, false, false)]
#[case(true, false, false)]
#[case(true, true, false)]
#[case(true, false, true)]
#[tokio::test]
async fn managed_external_write_requires_exact_one_call_approval(
	#[case] _expired: bool,
	#[case] late_answer: bool,
	#[case] _prior_response: bool,
	#[future(awt)]
	#[with(_expired, _prior_response)]
	write_approval: WriteApproval,
) {
	let WriteApproval {
		store,
		url,
		schema,
		harness,
		mut run,
		first,
		effects,
		server,
		_runtime,
	} = write_approval;
	assert_eq!(run.phase().as_str(), "WAITING");
	assert_eq!(effects.load(Ordering::SeqCst), 0);
	if !_expired {
		store
			.answer(first, json!({"approved":false}))
			.await
			.unwrap();
	}
	if late_answer {
		let result = store.answer(first, json!({"approved":true})).await.unwrap();
		assert_eq!(
			result.response,
			Some(json!({"approved":false,"expired":true})),
			"late answer must persist expiry"
		);
	}
	harness.worker_once().await.unwrap();
	harness.worker_once().await.unwrap();
	run = store.run(run.id).await.unwrap();
	assert_eq!(json!(run.state)["data"]["cursor"], 1);
	assert_eq!(effects.load(Ordering::SeqCst), 0);
	if _expired {
		let row: HumanRequest = {
			let query_bind_1 = first;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::cust("*"))
					.from(reinhardt::query::Alias::new("human_requests"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(store.pool.driver())
			.await
		}
		.unwrap();
		assert_eq!(row.response, Some(json!({"approved":false,"expired":true})));
		assert_eq!(row.answered_by.as_deref(), Some("system"));
		assert!(matches!(
			store.answer(first, json!({"approved":true})).await,
			Err(aidash_server::Error::Conflict(_))
		));
	}
	harness.worker_once().await.unwrap();
	run = store.run(run.id).await.unwrap();
	assert_eq!(run.phase().as_str(), "WAITING");
	let second = request_id(&run);
	assert_ne!(first, second);
	store
		.answer(second, json!({"approved":true}))
		.await
		.unwrap();
	harness.worker_once().await.unwrap();
	harness.worker_once().await.unwrap();
	assert_eq!(effects.load(Ordering::SeqCst), 1);
	drop(server);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn integration_failures_reach_the_agent_without_replay_or_schema_escape(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
	#[from(upstream_fixtures::hits)] source_hits: Arc<std::sync::atomic::AtomicUsize>,
	#[from(rejected_sources_router)]
	#[with(source_hits.clone())]
	_router: Arc<Router>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
) {
	use aidash_server::harness::Harness;

	let (store, database_url, schema) = _store_fixture.clone().await.parts();

	let endpoint = server.url.clone();
	let federation = _federation.await.federation;
	federation.registry.register(entry("model", "web-model", json!({"provider":"openrouter","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"credential_env":null,"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}))).await.unwrap();
	federation
		.registry
		.register({ let mut tool = entry(
			"tool",
			"web-fetch",
			json!({"registry_node":federation.config.node_id,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":format!("{endpoint}/evidence"),"credential_env":null,"replay":"read_only"}}),
		); tool.schema = json!({"type":"object","properties":{"url":{"type":"string","pattern":"^http://127\\.0\\.0\\.1:"}},"required":["url"],"additionalProperties":false}); tool })
		.await
		.unwrap();
	let agent = federation.registry.register(entry("agent", "web-research", json!({"model":{"id":"web-model","version":"1.0.0"},"instructions":"Use available public evidence","schema_version":1,"bindings":[{"kind":"tool","target":{"registry_node":federation.config.node_id,"id":"web-fetch","version":"1.0.0"},"alias":"plugin_0","narrow":{}}],"remove_default":[]}))).await.unwrap();
	let workspace = store
		.create_workspace("Web research", "Use available evidence")
		.await
		.unwrap();
	let task = running_task(&store, &agent, workspace.id, None).await;
	let harness = Harness { federation };
	// Four exact external approvals add durable waiting/resume transitions.
	for _ in 0..40 {
		let progress = harness.worker_once().await.unwrap();
		if approve_fixture_call(&store).await {
			continue;
		}
		if !progress {
			break;
		}
		if store.task(task.id).await.unwrap().status == aidash_server::domain::TaskStatus::Completed
		{
			break;
		}
	}
	let run = store.runs().await.unwrap().remove(0);
	assert_eq!(
		run.phase().as_str(),
		"COMPLETED",
		"error={:?}, pending={}, history={}",
		run.error,
		json!(run.state)["data"],
		json!(run.context.history)
	);
	assert_eq!(
		store.task(task.id).await.unwrap().status.as_str(),
		"COMPLETED"
	);
	assert_eq!(
		run.context
			.history
			.iter()
			.filter(|event| matches!(event, aidash_server::context::ContextEvent::Tool { .. }))
			.count(),
		4,
		"transport and schema errors each reach the next inference exactly once"
	);
	assert_eq!(
		source_hits.load(Ordering::SeqCst),
		3,
		"each permitted source is fetched once"
	);
	assert_eq!(
		store.snapshot(workspace.id).await.unwrap().artifacts[0].content,
		json!("Used alternate evidence")
	);
	assert!(
		!store
			.events(0, Some(workspace.id), 100)
			.await
			.unwrap()
			.iter()
			.any(|event| event.kind == "run.retrying")
	);
	drop(server);
	cleanup(store, &database_url, &schema).await;
}

async fn add_test_peer(store: &Store, node: &str, endpoint: &str) {
	{
		let query_bind_1 = node;
		let query_bind_2 = endpoint;
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("peers"))
				.columns([
					reinhardt::query::Alias::new("node_id"),
					reinhardt::query::Alias::new("endpoint"),
					reinhardt::query::Alias::new("credential_env"),
					reinhardt::query::Alias::new("protocol_version"),
					reinhardt::query::Alias::new("enabled"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"))
						.expr(reinhardt::query::Expr::cust("'0.2'"))
						.expr(reinhardt::query::Expr::cust("TRUE"))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
}
#[rstest::rstest]
#[tokio::test]
async fn failed_home_transition_survives_outage_and_worker_restart(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
	#[from(home_scene)]
	#[with(_federation.clone())]
	home_scene: HomeSceneFuture,
	#[from(upstream_fixtures::available)] online: Arc<std::sync::atomic::AtomicBool>,
	#[from(failed_home_router)]
	#[with(home_scene.clone(), online.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[future(awt)]
	#[from(upstream_fixtures::async_upstream)]
	#[with(_router.clone())]
	server: Arc<TestServerGuard>,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let scene = home_scene.await;
	let agent = scene.agent;
	let home_task = scene.task;
	let task = home_task.lock().unwrap().clone();

	let endpoint = server.url.clone();
	add_test_peer(&store, "aidash://home", &endpoint).await;
	let run = store
		.accept_run(&task, "aidash://home", &agent.id, &agent.version)
		.await
		.unwrap();
	{
		let query_bind_1 = run.id;
		let query_bind_2 = {
			let mut pending = common::pending(RunState::Thinking(Default::default()));
			pending["recovery"]["retry"] = json!(RetryState {
				count: 5,
				at: chrono::Utc::now() - chrono::Duration::seconds(1)
			});
			pending
		};
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'THINKING'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	aidash_server::harness::Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let pending = store.run(run.id).await.unwrap();
	assert_eq!(pending.phase().as_str(), "WAITING");
	assert_eq!(json!(pending.state)["data"]["target"], "FAILED");
	aidash_server::harness::Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	assert_eq!(
		store.inspect_run(run.id).await.unwrap().phase().as_str(),
		"WAITING"
	);
	assert_eq!(home_task.lock().unwrap().status.as_str(), "RUNNING");
	online.store(true, std::sync::atomic::Ordering::SeqCst);
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					reinhardt::query::Expr::cust(
						"JSONB_SET(pending, '{data,wake_at}', TO_JSONB(CURRENT_TIMESTAMP - INTERVAL '1 SECOND'))",
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	aidash_server::harness::Harness { federation: f }
		.worker_once()
		.await
		.unwrap();
	assert_eq!(
		store.inspect_run(run.id).await.unwrap().phase().as_str(),
		"FAILED"
	);
	assert_eq!(home_task.lock().unwrap().status.as_str(), "FAILED");
	drop(server);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn terminal_delegations_allow_reads_and_exact_completion_replay_only(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings::default(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), _federation.clone())]
	_application: common::ApplicationFixture,
	http_client: reqwest::Client,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let workspace = store
		.create_workspace("Revocation", "Reject stale peer writes")
		.await
		.unwrap();
	let task = store
		.create_task(workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	let peer = "aidash://peer";
	add_test_peer(&store, peer, "http://127.0.0.1:1").await;
	{
		let query_bind_1 = task.id;
		let query_bind_2 = peer;
		let query_bind_3 = &agent.id;
		let query_bind_4 = &agent.version;
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("delegations"))
				.columns([
					reinhardt::query::Alias::new("task_id"),
					reinhardt::query::Alias::new("node_id"),
					reinhardt::query::Alias::new("agent_id"),
					reinhardt::query::Alias::new("agent_version"),
					reinhardt::query::Alias::new("delivered"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("TRUE"))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	let owner = qualified_agent(peer, &agent.id, &agent.version);
	let task = store
		.claim(task.id, task.revision, &owner, &agent)
		.await
		.unwrap();
	store
		.transition(
			task.id,
			task.revision,
			&owner,
			aidash_server::domain::TaskStatus::Running,
		)
		.await
		.unwrap();
	let artifact = ArtifactInput {
		kind: "text".into(),
		name: "Final".into(),
		content: json!("Done"),
	};
	let key = format!("{peer}:{}:completion", task.id);
	store
		.complete(task.id, &owner, &key, &artifact)
		.await
		.unwrap();
	let router = _application.application.clone();
	let token = std::env::var("AIDASH_SECRET_TEST_PEER")
		.expect("set AIDASH_SECRET_TEST_PEER for peer regression tests");
	for operation in [
		"artifact",
		"create_task",
		"delegate",
		"message",
		"event",
		"human_message",
		"claim",
		"transition",
		"snapshot",
		"task",
		"complete",
	] {
		let data = if operation == "complete" {
			json!({"key":"completion","artifact":artifact})
		} else {
			json!({"key":"stale","content":"stale write","status":"RUNNING"})
		};
		let response = http_client.request(Method::POST, router.url("/federation/v0.1/workspace"))
		.header("authorization", format!("Bearer {token}"))
		.header("x-aidash-node", peer)
		.header("x-aidash-protocol", "0.2")
		.header("content-type", "application/json")
		.body(json!({"task_id":task.id,"agent":{"id":agent.id,"version":agent.version},"operation":operation,"data":data}).to_string())
		.send().await.unwrap();
		assert_eq!(
			response.status(),
			if matches!(operation, "snapshot" | "task" | "complete") {
				StatusCode::OK
			} else {
				StatusCode::UNAUTHORIZED
			},
			"{operation}"
		);
	}
	assert_eq!(
		store.snapshot(workspace.id).await.unwrap().artifacts.len(),
		1
	);
	let mut malformed = new_task();
	malformed.requirements = json!({"capabilty":"web.search"});
	assert!(
		store
			.create_task(workspace.id, &malformed, "human", None)
			.await
			.is_err()
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn queued_executor_conflict_rolls_back_claim_and_dependencies_wait(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let mut other = agent.clone();
	other.id = "other".into();
	other.binding_normalization = None;
	f.registry.register(other.clone()).await.unwrap();
	let workspace = store
		.create_workspace("Claims", "Do not strand work")
		.await
		.unwrap();
	let prerequisite = running_task(&store, &agent, workspace.id, None).await;
	{
		let query_bind_1 = prerequisite.id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("control"),
					reinhardt::query::Expr::cust("'PAUSED'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	let mut input = new_task();
	input.dependencies = vec![prerequisite.id];
	let task = store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let run = store
		.accept_run(&task, &store.node_id, &agent.id, &agent.version)
		.await
		.unwrap();
	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	harness.worker_once().await.unwrap();
	assert_eq!(store.run(run.id).await.unwrap().phase().as_str(), "WAITING");
	assert_eq!(store.task(task.id).await.unwrap().status.as_str(), "OPEN");
	store
		.complete(
			prerequisite.id,
			prerequisite.owner.as_deref().unwrap(),
			"prerequisite",
			&ArtifactInput {
				kind: "text".into(),
				name: "done".into(),
				content: json!("done"),
			},
		)
		.await
		.unwrap();
	let release_obligations: i64 = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("run_activations"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ? AND reason = 'dependency_release')".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		release_obligations, 1,
		"dependency completion must durably reactivate the waiting Run"
	);
	let owner = qualified_agent(&store.node_id, &other.id, &other.version);
	assert!(matches!(
		store.claim(task.id, task.revision, &owner, &other).await,
		Err(aidash_server::Error::Conflict(_))
	));
	let unchanged = store.task(task.id).await.unwrap();
	assert_eq!(unchanged.revision, task.revision);
	assert!(unchanged.owner.is_none());
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					reinhardt::query::Expr::cust(
						"JSONB_SET(pending, '{data,wake_at}', TO_JSONB(CURRENT_TIMESTAMP - INTERVAL '1 SECOND'))",
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	harness.worker_once().await.unwrap();
	harness.worker_once().await.unwrap();
	assert_eq!(
		store.task(task.id).await.unwrap().status.as_str(),
		"RUNNING"
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn child_creation_and_parent_completion_are_serialized(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let workspace = store
		.create_workspace("Children", "Atomic completion")
		.await
		.unwrap();
	let parent = running_task(&store, &agent, workspace.id, None).await;
	let mut input = new_task();
	input.parent_id = Some(parent.id);
	let artifact = ArtifactInput {
		kind: "text".into(),
		name: "parent".into(),
		content: json!("done"),
	};
	let (child, completed) = tokio::join!(
		store.create_task(workspace.id, &input, "human", Some("child")),
		store.complete(
			parent.id,
			parent.owner.as_deref().unwrap(),
			"parent",
			&artifact
		)
	);
	assert_ne!(child.is_ok(), completed.is_ok());
	if let Ok(child) = child {
		assert_eq!(
			store.task(parent.id).await.unwrap().status.as_str(),
			"RUNNING"
		);
		let cancelled = store
			.transition(
				child.id,
				child.revision,
				"human",
				aidash_server::domain::TaskStatus::Cancelled,
			)
			.await
			.unwrap();
		store
			.abandon_task(child.id, cancelled.revision, "No longer required")
			.await
			.unwrap();
		store
			.complete(
				parent.id,
				parent.owner.as_deref().unwrap(),
				"parent",
				&artifact,
			)
			.await
			.unwrap();
		assert_eq!(
			store
				.create_task(workspace.id, &input, "human", Some("child"))
				.await
				.unwrap()
				.id,
			child.id
		);
	}
	assert!(
		store
			.create_task(workspace.id, &input, "human", Some("new-child"))
			.await
			.is_err()
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn skill_reads_fit_the_pending_request_budget_before_recording(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	f.registry
		.register(entry(
			"skill",
			"bundled-skill",
			json!({"instructions":"Read references/guide.md","files":[{"path":"references/guide.md","content":"界".repeat(3000)}]}),
		))
		.await
		.unwrap();
	let mut agent = seed(&f.registry).await;
	agent.id = "skilled-agent".into();
	agent.binding_normalization = None;
	agent.config["bindings"] = json!([{"kind":"skill","target":{"registry_node":f.config.node_id,"id":"bundled-skill","version":"1.0.0"},"narrow":{}}]);
	f.registry.register(agent.clone()).await.unwrap();
	let workspace = store
		.create_workspace("Skill read", "Read a bundled guide")
		.await
		.unwrap();
	let task = running_task(&store, &agent, workspace.id, None).await;
	let response = aidash_server::provider::ModelResponse {
		tool_calls: vec![aidash_server::provider::ToolCall {
			id: "read-guide".into(),
			name: "skill_read".into(),
			arguments: json!({"skill":{"id":"bundled-skill","version":"1.0.0"},"path":"references/guide.md","offset":0,"max_chars":8000}),
		}],
		..Default::default()
	};
	{
		let query_bind_1 = task.id;
		let query_bind_2 = common::tool_pending(
			json!({"response":response,"cursor":0,"request_tokens":7500,"request_window":10000}),
		);
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	aidash_server::harness::Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let run = store.runs().await.unwrap().remove(0);
	let aidash_server::context::ContextEvent::Tool { result, .. } = &run.context.history[0] else {
		panic!("expected tool result")
	};
	let text = result["text"].as_str().unwrap();
	assert!(!text.is_empty() && text.len() < 8000);
	assert_eq!(result["budget_limited"], true);
	assert_eq!(result["next_offset"], text.chars().count());
	assert!(json!(run.state)["data"]["request_tokens"].as_u64().unwrap() <= 10000);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn unavailable_tools_are_results_and_child_gating_advances_step(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let _tracing =
		tracing::subscriber::set_default(tracing_subscriber::fmt().with_test_writer().finish());
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let workspace = store
		.create_workspace("Recovery", "Repair model errors")
		.await
		.unwrap();
	let parent = running_task(&store, &agent, workspace.id, None).await;
	let response = aidash_server::provider::ModelResponse {
		tool_calls: vec![aidash_server::provider::ToolCall {
			id: "bad".into(),
			name: "missing_tool".into(),
			arguments: json!({}),
		}],
		..Default::default()
	};
	{
		let query_bind_1 = parent.id;
		let query_bind_2 = common::tool_pending(json!({"response":response,"cursor":0}));
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	harness.worker_once().await.unwrap();
	let run: Run = {
		let query_bind_1 = parent.id;
		aidash_server::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(json!(run.state)["data"]["cursor"], 1);
	assert!(
		json!(run.context)["history"][0]["result"]["error"]
			.as_str()
			.unwrap()
			.contains("missing_tool")
	);
	let mut input = new_task();
	input.parent_id = Some(parent.id);
	let child = store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	final_response(&store, parent.id).await;
	harness.worker_once().await.unwrap();
	let waiting = store.run(run.id).await.unwrap();
	assert_eq!(waiting.phase().as_str(), "WAITING");
	assert_eq!(waiting.step, run.step + 1);
	let child = store
		.transition(
			child.id,
			child.revision,
			"human",
			aidash_server::domain::TaskStatus::Cancelled,
		)
		.await
		.unwrap();
	store
		.abandon_task(child.id, child.revision, "No longer required")
		.await
		.unwrap();
	let response = aidash_server::provider::ModelResponse {
		text: "A different final result after the child settled".into(),
		..Default::default()
	};
	{
		let query_bind_1 = run.id;
		let query_bind_2 = common::tool_pending(
			json!({"response":response,"response_epoch":waiting.step,"cursor":0}),
		);
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	harness.worker_once().await.unwrap();
	assert_eq!(
		store.task(parent.id).await.unwrap().status.as_str(),
		"COMPLETED"
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn peer_disable_and_retry_rotation_do_not_require_a_live_peer(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	add_test_peer(&store, "aidash://offline", "http://127.0.0.1:1").await;
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("peers"))
			.value_expr(
				reinhardt::query::Alias::new("credential_env"),
				reinhardt::query::Expr::cust("'AIDASH_SECRET_REMOVED'"),
			)
			.and_where(reinhardt::query::Expr::cust("node_id = 'aidash://offline'"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(store.pool.driver())
	.await
	.unwrap();
	let disabled = f
		.register_peer(aidash_server::federation::Peer {
			node_id: "aidash://offline".into(),
			endpoint: "http://127.0.0.1:1".into(),
			credential_env: "AIDASH_SECRET_REMOVED".into(),
			protocol_version: "0.2".into(),
			enabled: false,
		})
		.await
		.unwrap();
	assert!(!disabled.enabled);
	let workspace = store
		.create_workspace("Retry", "Healthy peers progress")
		.await
		.unwrap();
	let mut healthy = None;
	for index in 0..101 {
		let task = store
			.create_task(workspace.id, &new_task(), "human", None)
			.await
			.unwrap();
		let node = if index == 100 {
			&store.node_id
		} else {
			"aidash://offline"
		};
		{
			let query_bind_1 = task.id;
			let query_bind_2 = node;
			let query_bind_3 = &agent.id;
			let query_bind_4 = &agent.version;
			let query_bind_5 = index as f64;
			sqlx::query(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("delegations"))
					.columns([
						reinhardt::query::Alias::new("task_id"),
						reinhardt::query::Alias::new("node_id"),
						reinhardt::query::Alias::new("agent_id"),
						reinhardt::query::Alias::new("agent_version"),
						reinhardt::query::Alias::new("created_at"),
						reinhardt::query::Alias::new("next_attempt_at"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(CURRENT_TIMESTAMP + MAKE_INTERVAL(secs => ?))".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							))
							.expr(reinhardt::query::Expr::cust(
								"CURRENT_TIMESTAMP - INTERVAL '1 SECOND'",
							))
							.to_owned(),
					)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(store.pool.driver())
			.await
		}
		.unwrap();
		if index == 100 {
			healthy = Some(task.id);
		}
	}
	f.retry_deliveries().await.unwrap();
	f.retry_deliveries().await.unwrap();
	let delivered: bool = {
		let query_bind_1 = healthy;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("delivered")),
				))
				.from(reinhardt::query::Alias::new("delegations"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(store.pool.driver())
		.await
	}
	.unwrap();
	assert!(delivered);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn registry_event_and_conversation_creation_roll_back_as_units(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings::default(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), _federation.clone())]
	_application: common::ApplicationFixture,
	http_client: reqwest::Client,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	seed(&f.registry).await;
	let app = _application.application.clone();
	// Simulate an event insertion failure after the primary mutation.
	// SeaQuery cannot add a CHECK constraint to an existing table.
	sqlx::query(
		"ALTER TABLE events ADD CONSTRAINT reject_registration CHECK(kind <> 'registry.registered')",
	)
	.execute(store.pool.driver())
	.await
	.unwrap();
	let response = http_client
		.request(Method::POST, app.url("/api/registry"))
		.header("authorization", "Bearer test-access-token")
		.header("content-type", "application/json")
		.body(
			serde_json::to_string(&entry("skill", "rollback", json!({"instructions":"test"})))
				.unwrap(),
		)
		.send()
		.await
		.unwrap();
	let status = response.status();
	let body = response.text().await.unwrap();
	// Unexpected persistence failures keep the existing HTTP 500 envelope while
	// the primary mutation and its event roll back together.
	assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
	assert!(
		Registry::new(store.pool.clone(), &store.node_id)
			.unwrap()
			.get("rollback", "1.0.0")
			.await
			.is_err()
	);
	// SeaQuery cannot add a CHECK constraint to an existing table.
	sqlx::query(
		"ALTER TABLE events ADD CONSTRAINT reject_conversation CHECK(kind <> 'conversation.created')",
	)
	.execute(store.pool.driver())
	.await
	.unwrap();
	let response=http_client.request(Method::POST, app.url("/api/conversations"))
		.header("authorization","Bearer test-access-token")
		.header("content-type","application/json")
		.body(json!({"title":"Atomic","goal":"Must roll back","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}).to_string())
		.send().await.unwrap();
	assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
	assert!(store.workspaces().await.unwrap().is_empty());
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn oversized_outbox_payload_publishes_a_reference_without_blocking_later_events(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
	#[from(outbox_store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let mut f = _federation.await.federation;
	f.config.nats_url = _test_environment.nats_url.clone();
	let bus = aidash_server::bus::EventBus::connect(&f.config.nats_url, &store.node_id)
		.await
		.unwrap();
	let poison = store
		.emit(None, "large", json!({"text":"x".repeat(2_000_000)}))
		.await
		.unwrap();
	let healthy = store.emit(None, "small", json!({"ok":true})).await.unwrap();
	assert_eq!(bus.publish_once(&f).await.unwrap(), 2);
	let row: (bool, Option<String>) = {
		let query_bind_1 = poison.id;
		sqlx::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("published_at IS NOT NULL"))
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("publish_error")),
				))
				.from(reinhardt::query::Alias::new("events"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(store.pool.driver())
		.await
	}
	.unwrap();
	assert!(row.0);
	assert!(row.1.is_none());
	let stream = bus.context.get_stream(&bus.stream_name).await.unwrap();
	let mut saw_reference = false;
	for sequence in 1..=2 {
		let message = stream.get_raw_message(sequence).await.unwrap();
		let envelope: serde_json::Value = serde_json::from_slice(&message.payload).unwrap();
		if envelope["id"] == poison.id.to_string() {
			assert!(envelope.get("data").is_none());
			assert_eq!(
				envelope["dataref"],
				format!("/api/events?after={}", poison.sequence - 1)
			);
			let replay = store.events(poison.sequence - 1, None, 500).await.unwrap();
			assert_eq!(
				replay.iter().find(|e| e.id == poison.id).unwrap().data,
				poison.data
			);
			saw_reference = true;
		} else {
			assert_eq!(envelope["id"], healthy.id.to_string());
			assert_eq!(envelope["data"], healthy.data);
		}
	}
	assert!(saw_reference);
	assert_eq!(bus.publish_once(&f).await.unwrap(), 0);
	assert!(
		{
			let query_bind_1 = healthy.id;
			sqlx::query_scalar::<_, bool>(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::cust("published_at IS NOT NULL"))
					.from(reinhardt::query::Alias::new("events"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(store.pool.driver())
			.await
		}
		.unwrap()
	);
	bus.context.delete_stream(&bus.stream_name).await.unwrap();
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn ambiguous_peer_credentials_cannot_impersonate_another_node(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
	#[from(ambiguous_peer_credentials_cannot_impersonate_another_node_router)] _router: Arc<Router>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let secret = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
	add_test_peer(&store, "aidash://peer-b", "http://127.0.0.1:1").await;
	assert!(
		f.authenticate_peer("aidash://peer-b", &secret)
			.await
			.is_ok()
	);
	add_test_peer(&store, "aidash://peer-c", "http://127.0.0.1:1").await;
	assert!(
		f.authenticate_peer("aidash://peer-c", &secret)
			.await
			.is_err()
	);

	let endpoint = server.url.clone();
	assert!(matches!(
		f.register_peer(aidash_server::federation::Peer {
			node_id: "aidash://peer-d".into(),
			endpoint,
			credential_env: "AIDASH_SECRET_TEST_PEER".into(),
			protocol_version: "0.2".into(),
			enabled: true
		})
		.await,
		Err(aidash_server::Error::Invalid(_))
	));
	drop(server);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn recovery_publishes_reconciliation_marker_with_the_request(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let mut agent = seed(&f.registry).await;
	f.registry
		.register(entry(
			"tool",
			"unsafe-http",
			json!({"registry_node":f.config.node_id,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"http://127.0.0.1:9","replay":"unsafe"}}),
		))
		.await
		.unwrap();
	agent.id = "unsafe-agent".into();
	agent.binding_normalization = None;
	agent.config["bindings"] = json!([{"kind":"tool","target":{"registry_node":f.config.node_id,"id":"unsafe-http","version":"1.0.0"},"alias":"plugin_0","narrow":{}}]);
	f.registry.register(agent.clone()).await.unwrap();
	let workspace = store
		.create_workspace("Reconcile", "Never expose an incomplete request")
		.await
		.unwrap();
	let task = running_task(&store, &agent, workspace.id, None).await;
	let response = aidash_server::provider::ModelResponse {
		tool_calls: vec![aidash_server::provider::ToolCall {
			id: "unsafe".into(),
			name: "plugin_0".into(),
			arguments: json!({}),
		}],
		..Default::default()
	};
	{
		let query_bind_1 = task.id;
		let query_bind_2 = {
			let run = store.runs().await.unwrap().remove(0);
			let mut pending = common::tool_pending(json!({"response":response,"cursor":0}));
			// The crash follows approval and dispatch of this exact unsafe call.
			pending["data"]["workbench_approval_result"] = json!({
				"key":format!("{}:0:0", run.id), "call":response.tool_calls[0],
				"expires_at":chrono::Utc::now()+chrono::Duration::minutes(15), "approved":true
			});
			pending
		};
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	let worker = Uuid::new_v4();
	let run = store.lease_run(worker, 30).await.unwrap().unwrap();
	let key = format!("{}:0:0", run.id);
	store
		.invocation_start(&run, worker, &key, "plugin_0", &json!({}), false)
		.await
		.unwrap();
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("lease_until"),
					reinhardt::query::Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 SECOND'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
	.unwrap();
	aidash_server::harness::Harness { federation: f }
		.worker_once()
		.await
		.unwrap();
	let current = store.run(run.id).await.unwrap();
	let request = request_id(&current);
	assert_eq!(json!(current.state)["data"]["key"], key);
	assert!(matches!(
		store.answer(request, json!("done")).await,
		Err(aidash_server::Error::Invalid(_))
	));
	store
		.answer(request, json!({"result":"verified"}))
		.await
		.unwrap();
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn agent_tools_attach_children_and_clusters_require_existing_agents(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	use aidash_server::tool::{PluginTool, Tool, ToolConfig, ToolContext};
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	for coordinator in ["missing", "model"] {
		assert!(
			f.registry
				.register(entry(
					"cluster",
					coordinator,
					json!({"coordinator":{"id":coordinator,"version":"1.0.0"}})
				))
				.await
				.is_err()
		);
	}
	f.registry
		.register(entry(
			"cluster",
			"valid-cluster",
			json!({"coordinator":{"id":"research","version":"1.0.0"}}),
		))
		.await
		.unwrap();
	let workspace = store
		.create_workspace("Agent tool", "Await delegated results")
		.await
		.unwrap();
	let task = running_task(&store, &agent, workspace.id, None).await;
	let run: Run = {
		let query_bind_1 = task.id;
		aidash_server::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(store.pool.driver())
		.await
	}
	.unwrap();
	let config = ToolConfig::Agent {
		node_id: store.node_id.clone(),
		agent: aidash_server::registry::EntityRef {
			id: agent.id,
			version: agent.version,
		},
	};
	let plugin = PluginTool {
		entry: entry(
			"tool",
			"delegate-tool",
			serde_json::to_value(&config).unwrap(),
		),
		alias: "plugin_0".into(),
		config,
		client: f.client.clone(),
	};
	let context = ToolContext {
		home: aidash_server::federation::Home::new(f, run.clone()),
		store: store.clone(),
		run,
	};
	for (index, parent) in [None, Some(serde_json::Value::Null)]
		.into_iter()
		.enumerate()
	{
		let mut input = json!({"title":"Child","description":"Delegated work"});
		if let Some(parent) = parent {
			input["parent_id"] = parent;
		}
		let result = plugin
			.invoke(&context, input, &format!("delegated-{index}"))
			.await
			.unwrap();
		assert_eq!(result["task"]["parent_id"], task.id.to_string());
	}
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn terminal_dependencies_fail_dependents_instead_of_polling_forever(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let worker = aidash_server::harness::Harness { federation: f };
	for terminal in ["FAILED", "CANCELLED", "ABANDONED"] {
		let workspace = store
			.create_workspace("Dependency", terminal)
			.await
			.unwrap();
		let dependency = running_task(&store, &agent, workspace.id, None).await;
		{
			let query_bind_1 = dependency.id;
			sqlx::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value_expr(
						reinhardt::query::Alias::new("control"),
						reinhardt::query::Expr::cust("'PAUSED'"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(store.pool.driver())
			.await
		}
		.unwrap();
		{
			let query_bind_1 = dependency.id;
			let query_bind_2 = terminal;
			sqlx::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("tasks"))
					.value_expr(
						reinhardt::query::Alias::new("status"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(store.pool.driver())
			.await
		}
		.unwrap();
		let mut input = new_task();
		input.dependencies = vec![dependency.id];
		let task = store
			.create_task(workspace.id, &input, "human", None)
			.await
			.unwrap();
		let run = store
			.accept_run(&task, &store.node_id, &agent.id, &agent.version)
			.await
			.unwrap();
		// Terminal failure delivery is scheduled with a database wake time;
		// consecutive polls need not straddle that clock boundary.
		tokio::time::timeout(std::time::Duration::from_secs(5), async {
			loop {
				worker.worker_once().await.unwrap();
				if store.run(run.id).await.unwrap().phase().as_str() == "FAILED" {
					break;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("terminal dependency must settle rather than wait indefinitely");
		let run = store.run(run.id).await.unwrap();
		assert_eq!(run.phase().as_str(), "FAILED");
		assert!(run.error.unwrap().contains(terminal));
		assert_eq!(store.task(task.id).await.unwrap().status.as_str(), "FAILED");
		assert!(!worker.worker_once().await.unwrap());
	}
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_workspace_snapshot_pages_large_accumulated_artifacts(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(store)]
	#[with("aidash://worker")]
	_store_fixture_2: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_home_runtime: common::RuntimeFuture,
	#[from(federation)]
	#[with(_store_fixture_2.clone())]
	_worker_runtime: common::RuntimeFuture,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings::default(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), _home_runtime.clone())]
	_home_app: common::ApplicationFixture,
) {
	let (home, url, schema) = _store_fixture.clone().await.parts();
	let f = _home_runtime.await.federation;
	let agent = seed(&f.registry).await;
	let workspace = home
		.create_workspace("Large snapshot", "Preserve every item")
		.await
		.unwrap();
	let task = running_task(&home, &agent, workspace.id, None).await;
	for index in 0..3 {
		home.publish_artifact(
			task.id,
			task.owner.as_deref().unwrap(),
			&format!("large-{index}"),
			&ArtifactInput {
				kind: "text".into(),
				name: format!("Large {index}"),
				content: json!("a".repeat(2_000_000)),
			},
		)
		.await
		.unwrap();
	}
	for _ in 0..35 {
		home.create_task(workspace.id, &new_task(), "human", None)
			.await
			.unwrap();
	}
	let full = home.snapshot(workspace.id).await.unwrap();
	assert!(serde_json::to_vec(&full).unwrap().len() > 4_194_304);
	let page = home
		.snapshot_page(workspace.id, "artifacts", None)
		.await
		.unwrap();
	assert!(serde_json::to_vec(&page).unwrap().len() < 3_145_728);
	assert_eq!(page.items.len(), 1);
	assert!(page.next.is_some());
	let (worker_store, worker_url, worker_schema) = _store_fixture_2.clone().await.parts();
	seed(&Registry::new(worker_store.pool.clone(), &worker_store.node_id).unwrap()).await;
	let server = _home_app.application;
	let endpoint = server.server.url.clone();
	add_test_peer(&home, &worker_store.node_id, "http://127.0.0.1:9").await;
	add_test_peer(&worker_store, &home.node_id, &endpoint).await;
	{
		let query_bind_1 = task.id;
		let query_bind_2 = &worker_store.node_id;
		let query_bind_3 = &agent.id;
		let query_bind_4 = &agent.version;
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("delegations"))
				.columns([
					reinhardt::query::Alias::new("task_id"),
					reinhardt::query::Alias::new("node_id"),
					reinhardt::query::Alias::new("agent_id"),
					reinhardt::query::Alias::new("agent_version"),
					reinhardt::query::Alias::new("delivered"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("TRUE"))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(home.pool.driver())
		.await
	}
	.unwrap();

	let run = worker_store
		.accept_run(&task, &home.node_id, &agent.id, &agent.version)
		.await
		.unwrap();
	let remote = aidash_server::federation::Home::new(_worker_runtime.await.federation, run)
		.snapshot()
		.await
		.unwrap();
	assert_eq!(remote.tasks.len(), full.tasks.len());
	assert_eq!(remote.artifacts.len(), 3);
	assert_eq!(
		remote
			.artifacts
			.iter()
			.map(|item| item.content.as_str().unwrap().len())
			.sum::<usize>(),
		6_000_000
	);
	drop(server);
	cleanup(worker_store, &worker_url, &worker_schema).await;
	cleanup(home, &url, &schema).await;
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use reinhardt::query::SimpleExpr;

fn request_id(run: &Run) -> Uuid {
	let RunState::Waiting(waiting) = &run.state else {
		panic!("expected waiting execution")
	};
	waiting
		.request_id()
		.expect("expected waiting human request")
}

#[path = "postgres/typed_state.rs"]
mod typed_state;

#[rstest::fixture]
fn successful_tool_retry_resets_the_next_invocation_budget_router() -> Arc<Router> {
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/",
				http::Method::POST,
				reply(|request: reinhardt::Request| {
					let input = request.json::<serde_json::Value>().unwrap();
					async move {
						if input["call"] == 1 {
							reinhardt::Response::new(StatusCode::OK)
								.with_json(&json!({"saved":true}))
								.unwrap()
						} else {
							reinhardt::Response::new(StatusCode::SERVICE_UNAVAILABLE)
								.with_json(&json!({"error":"temporary failure"}))
								.unwrap()
						}
					}
				}),
			)
			.into_server_router(),
	)
}

#[rstest::fixture]
fn write_approval_router(
	#[from(upstream_fixtures::hits)] effects: Arc<std::sync::atomic::AtomicUsize>,
) -> Arc<Router> {
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/",
				http::Method::POST,
				reply(move |_request: reinhardt::Request| {
					let counter = effects.clone();
					async move {
						counter.fetch_add(1, Ordering::SeqCst);
						reinhardt::Response::ok()
							.with_json(&json!({"ok":true}))
							.unwrap()
					}
				}),
			)
			.into_server_router(),
	)
}

#[rstest::fixture]
fn ambiguous_peer_credentials_cannot_impersonate_another_node_router() -> Arc<Router> {
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/.well-known/aidash",
				http::Method::GET,
				reply(|_request: reinhardt::Request| async {
					reinhardt::Response::ok()
						.with_json(&json!({"id":"aidash://peer-d","protocol_version":"0.2"}))
						.unwrap()
				}),
			)
			.into_server_router(),
	)
}

#[fixture]
fn rejected_sources_router(
	#[from(upstream_fixtures::hits)] source_hits: Arc<std::sync::atomic::AtomicUsize>,
) -> Arc<Router> {
	Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/evidence", http::Method::POST, reply(move |request: reinhardt::Request| {
            let body = request.json::<serde_json::Value>().unwrap();
            let hits = source_hits.clone();
            async move {
                hits.fetch_add(1,Ordering::SeqCst);
                let url = reqwest::Url::parse(body["url"].as_str().unwrap()).unwrap();
                let result = match url.path() {
                    "/blocked" => json!({"ok":false,"error":{"kind":"source_status","status":403}}),
                    "/missing" => json!({"ok":false,"error":{"kind":"source_status","status":404}}),
                    "/alternate" => json!({"ok":true,"text":"Alternate evidence"}),
                    _ => panic!("unexpected source"),
                };
                reinhardt::Response::ok().with_json(&result).unwrap()
            }
        }))
.route("/v1/chat/completions", http::Method::POST, reply(move |request: reinhardt::Request| {let body = request.json::<serde_json::Value>().unwrap();
			let endpoint = format!("http://{}", request.headers["host"].to_str().unwrap());
let forbidden_host = format!("http://localhost:{}/blocked", reqwest::Url::parse(&endpoint).unwrap().port().unwrap());
			async move {
				let context: serde_json::Value = serde_json::from_str(
					body["messages"][1]["content"].as_str().unwrap()
				).unwrap();
				let history: Vec<_> = context["history"].as_array().unwrap().iter().filter(|event|event.get("call").is_some()).collect();
				let next = match history.len() {
					0 => Some(format!("{endpoint}/blocked")),
					1 => {
						assert_eq!(history[0]["result"]["ok"], false);
						assert_eq!(history[0]["result"]["error"]["kind"], "source_status");
						assert_eq!(history[0]["result"]["error"]["status"], 403);
						Some(format!("{endpoint}/missing"))
					}
					2 => {
						assert_eq!(history[1]["result"], json!({"ok":false,"error":{"kind":"source_status","status":404}}));
						Some(format!("{endpoint}/alternate"))
					}
					3 => {
						assert_eq!(history[2]["result"]["text"], "Alternate evidence");
						Some(forbidden_host)
					}
					4 => {
						assert!(history[3]["result"]["error"].as_str().unwrap().contains("does not match"));
						None
					}
					_ => panic!("unexpected inference after final response"),
				};
				let message = if let Some(url) = next {
					json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("fetch-{}",history.len()),"type":"function","function":{"name":"plugin_0","arguments":json!({"url":url}).to_string()}}]})
				} else {
					json!({"role":"assistant","content":"Used alternate evidence"})
				};
				reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some() {"tool_calls"} else {"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
			}
		})).into_server_router())
}

type HomeSceneFuture = Shared<BoxFuture<'static, HomeScene>>;
#[derive(Clone)]
struct HomeScene {
	agent: Entry,
	task: Arc<std::sync::Mutex<Task>>,
	_runtime: common::RuntimeFixture,
}
#[fixture]
fn home_scene(#[from(federation)] runtime: common::RuntimeFuture) -> HomeSceneFuture {
	async move {
		let owner = runtime.await;
		let f = &owner.federation;
		let store = &f.store;

		let agent = seed(&f.registry).await;
		let workspace = store
			.create_workspace("Remote failure", "Retry terminal delivery")
			.await
			.unwrap();
		let mut task = store
			.create_task(workspace.id, &new_task(), "human", None)
			.await
			.unwrap();
		task.status = TaskStatus::Running;
		task.owner = Some(qualified_agent(&store.node_id, &agent.id, &agent.version));
		HomeScene {
			agent,
			task: Arc::new(std::sync::Mutex::new(task)),
			_runtime: owner,
		}
	}
	.boxed()
	.shared()
}
#[fixture]
fn failed_home_router(
	home_scene: HomeSceneFuture,
	#[from(upstream_fixtures::available)] online: Arc<std::sync::atomic::AtomicBool>,
) -> upstream_fixtures::RouterFuture {
	async move {
		let remote_task = home_scene.await.task;
		let available = online;
		Arc::new(
			reinhardt::test::stub::StubRouter::new()
				.route(
					"/federation/v0.1/workspace",
					http::Method::POST,
					reply(move |request: reinhardt::Request| {
						let body = request.json::<serde_json::Value>().unwrap();
						let task = remote_task.clone();
						let available = available.clone();
						async move {
							if !available.load(std::sync::atomic::Ordering::SeqCst) {
								return reinhardt::Response::new(StatusCode::SERVICE_UNAVAILABLE)
									.with_json(&json!({"error":"home unavailable"}))
									.unwrap();
							}
							let mut task = task.lock().unwrap();
							match body["operation"].as_str() {
								Some("task") => {}
								Some("run_message_terminal_transition") => {
									task.status =
										serde_json::from_value(body["data"]["status"].clone())
											.unwrap();
								}
								_ => {
									return reinhardt::Response::new(StatusCode::BAD_REQUEST)
										.with_json(&json!({"error":"unknown federation operation"}))
										.unwrap();
								}
							}
							reinhardt::Response::new(StatusCode::OK)
								.with_json(&json!(*task))
								.unwrap()
						}
					}),
				)
				.into_server_router(),
		)
	}
	.boxed()
	.shared()
}

#[fixture]
fn authenticated_runtime(
	#[from(store)] store_fixture: StoreFuture,
	http_client: reqwest::Client,
) -> common::RuntimeFuture {
	async move {
		let store_fixture = store_fixture.await;
		let mut runtime = store_fixture.runtime;
		let store = store_fixture.store;
		runtime.federation.store = store.clone();
		runtime.federation.registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
		runtime.federation.config = Config {
			node_id: store.node_id.clone(),
			endpoint: "http://127.0.0.1:18080".into(),
			database_url: runtime.url.clone(),
			nats_url: "nats://127.0.0.1:42270".into(),
			api_token: "test-access-token".into(),
			web_dir: "web/dist".into(),
			lease_seconds: 30,
			default_host_packages: vec![],
			oidc: None,
			gcip: None,
		};
		runtime.federation.client = http_client;
		runtime
	}
	.boxed()
	.shared()
}

#[rstest::rstest]
#[tokio::test]
async fn operator_remote_humans_live_on_home_and_survive_receiver_reconstruction(
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://human-home")]
	home_fixture: common::PeerFixture,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://human-worker")]
	worker_fixture: common::PeerFixture,
	#[from(reinhardt::test::fixtures::http_client)] transport: reqwest::Client,
) {
	let (home_f, home_url, home_schema) = home_fixture.runtime.parts();
	let (worker_f, worker_url, worker_schema) = worker_fixture.runtime.parts();
	let home_store = home_f.store.clone();
	let worker_store = worker_f.store.clone();
	home_f.registry.seed_system().await.unwrap();
	let agent = seed(&worker_f.registry).await;
	let home_app = home_fixture.application.clone();
	let worker_app = worker_fixture.application.clone();
	add_test_peer(
		&home_store,
		&worker_store.node_id,
		&worker_f.config.endpoint,
	)
	.await;
	add_test_peer(&worker_store, &home_store.node_id, &home_f.config.endpoint).await;
	let workspace = home_store
		.create_workspace("Remote decisions", "Keep decisions at Home")
		.await
		.unwrap();
	let task = home_store
		.create_task(workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	let insert = Query::insert()
		.into_table(Alias::new("delegations"))
		.columns(["task_id", "node_id", "agent_id", "agent_version"].map(Alias::new))
		.values_panic([
			reinhardt::query::IntoValue::into_value(task.id),
			reinhardt::query::IntoValue::into_value(worker_store.node_id.clone()),
			reinhardt::query::IntoValue::into_value(agent.id.clone()),
			reinhardt::query::IntoValue::into_value(agent.version.clone()),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert)
		.execute(home_store.pool.driver())
		.await
		.unwrap();
	let owner = qualified_agent(&worker_store.node_id, &agent.id, &agent.version);
	let task = home_store
		.claim(task.id, task.revision, &owner, &agent)
		.await
		.unwrap();
	let task = home_store
		.transition(task.id, task.revision, &owner, TaskStatus::Running)
		.await
		.unwrap();
	let offer = json!({"task":task,"agent":{"id":agent.id,"version":agent.version}});
	let old = transport
		.post(worker_app.url("/federation/v0.1/offers"))
		.bearer_auth(std::env::var("AIDASH_SECRET_TEST_PEER").unwrap())
		.header("x-aidash-node", &home_store.node_id)
		.header("x-aidash-protocol", "0.1")
		.json(&offer)
		.send()
		.await
		.unwrap();
	assert!(
		!old.status().is_success(),
		"unsupported Home protocol must not admit a Run"
	);
	let run: Run = home_f
		.request(&worker_store.node_id, Method::POST, "/offers", Some(&offer))
		.await
		.unwrap();
	let home = aidash_server::federation::Home::new(worker_f.clone(), run.clone());
	assert_eq!(home.snapshot().await.unwrap().workspace.id, workspace.id);
	let question = home
		.human_request("QUESTION", "Continue?", "remote:question")
		.await
		.unwrap();
	assert!(
		home_store.run(run.id).await.is_err(),
		"Home must not acquire a shadow execution Run"
	);
	assert!(
		aidash_application::ports::execution::ExecutionStore::human_request_by_id(
			&home_store,
			question.id
		)
		.await
		.is_err()
	);
	let recovered = aidash_server::federation::Home::new(
		worker_f.clone(),
		worker_store.run(run.id).await.unwrap(),
	);
	assert_eq!(
		recovered
			.human_request("QUESTION", "Continue?", "remote:question")
			.await
			.unwrap()
			.id,
		question.id
	);
	assert!(
		recovered
			.human_request("QUESTION", "Changed?", "remote:question")
			.await
			.is_err()
	);
	let answered = common::request(
		&worker_app,
		&worker_f.config.api_token,
		"POST",
		&format!("/api/human-requests/{}/answer", question.id),
		json!({"answer":"yes"}),
	)
	.await;
	assert_eq!(answered.0, 200, "{}", answered.1);
	assert_eq!(
		recovered
			.human_request_by_id(question.id)
			.await
			.unwrap()
			.response,
		Some(json!({"answer":"yes"}))
	);
	let next = recovered
		.human_request("CONFIRMATION", "Again?", "remote:next")
		.await
		.unwrap();
	let answer: HumanRequest = home_f
		.request(
			&worker_store.node_id,
			Method::POST,
			"/control",
			Some(&json!({"run_id":run.id,"action":"answer","request_id":next.id,"response":false})),
		)
		.await
		.unwrap();
	assert_eq!(answer.response, Some(json!(false)));
	assert_eq!(
		recovered
			.human_request_by_id(next.id)
			.await
			.unwrap()
			.response,
		Some(json!(false))
	);
	let mut foreign = run.clone();
	foreign.id = Uuid::new_v4();
	let foreign = aidash_server::federation::Home::new(worker_f, foreign);
	assert!(foreign.human_request_by_id(question.id).await.is_err());
	assert!(
		foreign
			.human_request("QUESTION", "Other Run?", "remote:other")
			.await
			.is_err()
	);
	let task = home_store.task(task.id).await.unwrap();
	home_store
		.transition(task.id, task.revision, &owner, TaskStatus::Cancelled)
		.await
		.unwrap();
	assert!(
		recovered
			.human_request("QUESTION", "After cancellation?", "remote:cancelled")
			.await
			.is_err()
	);
	drop((home_app, worker_app));
	cleanup(worker_store, &worker_url, &worker_schema).await;
	cleanup(home_store, &home_url, &home_schema).await;
}

#[rstest::fixture]
fn outbox_store(#[from(common::runtime)] runtime: common::RuntimeFuture) -> StoreFuture {
	let node = format!("aidash://outbox-{}", Uuid::new_v4().simple());
	store(&node, runtime)
}
