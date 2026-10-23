#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
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
	registry::{Entry, Registry, Search},
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

static ACME: std::sync::LazyLock<aidash_application::tenancy::TenancyScope> =
	std::sync::LazyLock::new(|| aidash_application::tenancy::TenancyScope::tenant("acme").unwrap());

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
	let node_id = aidash_domain::nodes::DeploymentId::parse(node_id).unwrap();
	async move {
		let mut runtime = runtime.await;
		runtime.federation.store.deployment = node_id.clone();
		Registry::new(runtime.federation.store.pool.clone(), &node_id)
			.unwrap()
			.seed_system()
			.await
			.unwrap();
		common::tenant_scope(&runtime.federation.store, "acme").await;
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
	registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry("model","model",json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:9999/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}))).await.unwrap();
	registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry("agent","research",json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Research","schema_version":1,"bindings":[],"remove_default":[]}))).await.unwrap()
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
	let registry = Registry::new(store.pool.clone(), &store.deployment).unwrap();
	let agent = seed(&registry).await;
	let w = store
		.create_workspace(&ACME, "alice", "Research", "Compare frameworks")
		.await
		.unwrap();
	let task = store
		.create_task(&ACME, w.id, &new_task(), "human", Some("create"))
		.await
		.unwrap();
	assert_eq!(
		task.id,
		store
			.create_task(&ACME, w.id, &new_task(), "human", Some("create"))
			.await
			.unwrap()
			.id
	);
	let mut different = new_task();
	different.title = "Different".into();
	assert!(
		store
			.create_task(&ACME, w.id, &different, "human", Some("create"))
			.await
			.is_err()
	);
	let owner = qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme"),
		&agent.id,
		&agent.version,
	);
	let (a, b) = tokio::join!(
		store.claim(&ACME, task.id, 0, &owner, &agent),
		store.claim(&ACME, task.id, 0, &owner, &agent)
	);
	assert_ne!(a.is_ok(), b.is_ok());
	let claimed = store.task(&ACME, task.id).await.unwrap();
	assert_eq!(claimed.revision, 1);
	let mut dependent = new_task();
	dependent.dependencies = vec![task.id];
	// A prerequisite and its dependent are siblings, not a parent/child cycle.
	dependent.parent_id = None;
	let child = store
		.create_task(&ACME, w.id, &dependent, "human", None)
		.await
		.unwrap();
	assert!(
		store
			.claim(&ACME, child.id, 0, &owner, &agent)
			.await
			.is_err()
	);
	assert!(
		store
			.complete(
				&ACME,
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
			&ACME,
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
		.complete(&ACME, task.id, &owner, "complete", &artifact)
		.await
		.unwrap();
	let replay = store
		.complete(&ACME, task.id, &owner, "complete", &artifact)
		.await
		.unwrap();
	assert_eq!(completed.revision, replay.revision);
	assert_eq!(
		store.snapshot(&ACME, w.id).await.unwrap().artifacts.len(),
		1
	);
	let mut bad = artifact.clone();
	bad.content = json!("different");
	assert!(
		store
			.complete(&ACME, task.id, &owner, "complete", &bad)
			.await
			.is_err()
	);
	assert!(
		store
			.claim(&ACME, child.id, 0, &owner, &agent)
			.await
			.is_ok()
	);
	let w2 = store
		.create_workspace(&ACME, "alice", "Other", "Other goal")
		.await
		.unwrap();
	assert!(
		store
			.create_task(&ACME, w2.id, &dependent, "human", None)
			.await
			.is_err()
	);
	let events = store.events(&ACME, 0, Some(w.id), 1000).await.unwrap();
	assert_eq!(
		events.iter().filter(|e| e.kind == "task.completed").count(),
		1
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn salted_run_creation_requires_a_cache_salt_key(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
	#[values("ordered", "native")] version: &str,
) {
	// Arrange: a model that declares the salted version and an Agent that
	// names it, on a node without Cache Salt Keys.
	let (store, url, schema) = _store_fixture.parts();
	let registry = Registry::new(store.pool.clone(), &store.deployment).unwrap();
	registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry("model","model",json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:9999/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{},"projection_versions":["legacy","ordered","native"]}))).await.unwrap();
	let agent = registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry("agent","research",json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Research","schema_version":1,"bindings":[],"remove_default":[],"projection_version":version}))).await.unwrap();
	let workspace = store
		.create_workspace(&ACME, "alice", "Research", "Compare frameworks")
		.await
		.unwrap();
	let task = store
		.create_task(&ACME, workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	let owner = qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme"),
		&agent.id,
		&agent.version,
	);
	// Act: claiming would create the Run.
	let rejected = store.claim(&ACME, task.id, 0, &owner, &agent).await;
	// Assert: no Run and no claim without a key.
	assert!(
		matches!(rejected, Err(aidash_server::Error::Invalid(_))),
		"{rejected:?}"
	);
	assert_eq!(store.task(&ACME, task.id).await.unwrap().revision, 0);
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
	let salted_claim = salted
		.claim(&ACME, task.id, 0, &owner, &agent)
		.await
		.unwrap();
	assert_eq!(salted_claim.revision, 1);
	let other = store
		.create_task(&ACME, workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	let legacy = registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry("agent","legacy-research",json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Research","schema_version":1,"bindings":[],"remove_default":[]}))).await.unwrap();
	let legacy_owner = qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme"),
		&legacy.id,
		&legacy.version,
	);
	let claimed = store
		.claim(&ACME, other.id, 0, &legacy_owner, &legacy)
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
	let registry = Registry::new(store.pool.clone(), &store.deployment).unwrap();
	let agent = seed(&registry).await;
	let w = store
		.create_workspace(&ACME, "alice", "Journal", "Recover safely")
		.await
		.unwrap();
	let task = store
		.create_task(&ACME, w.id, &new_task(), "human", None)
		.await
		.unwrap();
	let run = common::local_run(&store, &ACME, &task, &agent).await;
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
		&store.for_run(ACME.clone()),
		&mut leased,
		token,
	)
	.await
	.unwrap();
	assert_eq!(leased.revision, revision + 1);
	assert_eq!(
		store
			.run(&ACME, run.id)
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
			&ACME,
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
		.execute(store.pool.fixture_driver())
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
			&store.for_run(ACME.clone()),
			&mut leased,
			token
		)
		.await
		.is_err()
	);
	assert!(
		store
			.invocation_finish(&ACME, &leased, token, "effect", &json!("stale"))
			.await
			.is_err()
	);
	let uncertain = store
		.invocation_start(
			&ACME,
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
			&ACME,
			&recovered,
			"CONFIRMATION",
			"Verify the external result",
			"effect:reconcile",
		)
		.await
		.unwrap();
	store
		.answer(&ACME, request.id, json!({"result":"verified"}))
		.await
		.unwrap();
	store
		.invocation_finish(&ACME, &recovered, new_token, "effect", &json!("verified"))
		.await
		.unwrap();
	let replay = store
		.invocation_start(
			&ACME,
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
				&ACME,
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
async fn registry_versions_and_authenticated_api(
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
	let registry = Registry::new(store.pool.clone(), &store.deployment).unwrap();
	seed(&registry).await;
	let skill = entry(
		"skill",
		"research-guide",
		json!({"instructions":"Keep citations"}),
	);
	registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			skill.clone(),
		)
		.await
		.unwrap();
	assert_eq!(
		registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				&skill.id,
				&skill.version
			)
			.await
			.unwrap(),
		skill
	);
	let mut changed = skill;
	changed.description.insert("en".into(), "Changed".into());
	assert!(
		registry
			.register(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				changed
			)
			.await
			.is_err()
	);
	let matches = registry
		.list(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			&Search {
				kind: Some("agent".into()),
				capability: Some("web.search".into()),
				language: Some("ja".into()),
				..Default::default()
			},
		)
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
		.request(Method::POST, router.url("/federation/v0.1/scoped/discover"))
		.header("x-aidash-protocol", aidash_server::config::PROTOCOL_VERSION)
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
async fn human_requests_controls_and_cancellation_keep_decisions_durable(
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
	let registry = Registry::new(store.pool.clone(), &store.deployment).unwrap();
	let agent = seed(&registry).await;
	let workspace = store
		.create_workspace(&ACME, "alice", "Controls", "Keep human decisions durable")
		.await
		.unwrap();
	let task = store
		.create_task(&ACME, workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	let federation = _api_runtime.await.federation;
	let run = common::admitted_run(&federation, "acme", &task, &agent).await;
	let token = common::tenant_token(&store, "acme").await;
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
		.execute(store.pool.fixture_driver())
		.await
		.unwrap();
	let explicitly_paused = store
		.control(
			&ACME,
			run.id,
			aidash_server::domain::RunControlAction::Pause,
		)
		.await
		.unwrap();
	assert_eq!(explicitly_paused.control.as_str(), "PAUSED");
	assert_eq!(explicitly_paused.error, None);
	assert!(!harness.worker_once().await.unwrap());
	assert_eq!(
		store
			.control(
				&ACME,
				run.id,
				aidash_server::domain::RunControlAction::Resume
			)
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
			.human_request(&ACME, &run, kind, "Proceed?", kind)
			.await
			.unwrap();
		assert_eq!(
			store
				.human_request(&ACME, &run, kind, "Proceed?", kind)
				.await
				.unwrap()
				.id,
			request.id
		);
		assert!(
			store
				.human_request(&ACME, &run, kind, "Changed?", kind)
				.await
				.is_err()
		);
		assert!(store.answer(&ACME, request.id, json!(null)).await.is_err());
		assert_eq!(
			store
				.answer(&ACME, request.id, json!(false))
				.await
				.unwrap()
				.response,
			Some(json!(false))
		);
		assert_eq!(
			store
				.answer(&ACME, request.id, json!(false))
				.await
				.unwrap()
				.id,
			request.id
		);
		assert!(store.answer(&ACME, request.id, json!(true)).await.is_err());
	}
	let router = _application.application.clone();
	let response = http_client
		.request(
			Method::POST,
			router.url(format!("/api/runs/{}/message", run.id)),
		)
		.header("Authorization", format!("Bearer {token}"))
		.header("content-type", "application/json")
		.body(r#"{"content":"Keep the source citations."}"#)
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::OK);
	let snapshot = store.snapshot(&ACME, workspace.id).await.unwrap();
	assert_eq!(snapshot.messages[0].sender, "alice");
	store
		.control(
			&ACME,
			run.id,
			aidash_server::domain::RunControlAction::Cancel,
		)
		.await
		.unwrap();
	assert!(harness.worker_once().await.unwrap());
	assert_eq!(
		store.run(&ACME, run.id).await.unwrap().phase().as_str(),
		"CANCELLED"
	);
	assert_eq!(
		store.task(&ACME, task.id).await.unwrap().status.as_str(),
		"CANCELLED"
	);
	assert!(
		store
			.control(
				&ACME,
				run.id,
				aidash_server::domain::RunControlAction::Resume
			)
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
			store: store.clone(),
			registry: Registry::new(store.pool.clone(), &store.deployment).unwrap(),
			config: Config {
				deployment: store.deployment.clone(),
				endpoint: "http://127.0.0.1:18080".into(),
				database_url: store.pool.connect_options().to_url_lossy().to_string(),
				nats_url: "nats://127.0.0.1:1".into(),
				api_token: "test-access-token".into(),
				web_dir: "web/dist".into(),
				lease_seconds: 30,
				tenancy: aidash_domain::identity::tenancy::TenancyRules::of(
					aidash_domain::offering::Offering::SelfHosted,
				),
				demo_data: false,
				oidc: None,
				sign_in: None,
				federation: true,
				deployment_key: None,
				tier_limits: std::sync::Arc::new(aidash_application::ports::nodes::NoTierLimits),
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

async fn running_task(
	f: &Federation,
	agent: &Entry,
	workspace: Uuid,
	parent: Option<Uuid>,
) -> Task {
	let store = &f.store;
	let mut input = new_task();
	input.parent_id = parent;
	let task = store
		.create_task(&ACME, workspace, &input, "alice", None)
		.await
		.unwrap();
	let owner = qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme"),
		&agent.id,
		&agent.version,
	);
	if parent.is_some() {
		store
			.claim(&ACME, task.id, task.revision, &owner, agent)
			.await
			.unwrap();
	} else {
		common::admitted_run(f, "acme", &task, agent).await;
	}
	let task = store.task(&ACME, task.id).await.unwrap();
	store
		.transition(
			&ACME,
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
		.execute(store.pool.fixture_driver())
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
		.create_workspace(
			&ACME,
			"alice",
			"Partial results",
			"Resolve terminal children",
		)
		.await
		.unwrap();
	let parent = running_task(&f, &agent, workspace.id, None).await;
	let mut children = Vec::new();
	for status in [
		TaskStatus::Failed,
		TaskStatus::Blocked,
		TaskStatus::Cancelled,
	] {
		let child = running_task(&f, &agent, workspace.id, Some(parent.id)).await;
		let child = store
			.transition(
				&ACME,
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
			.execute(store.pool.fixture_driver())
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
		.fetch_one(store.pool.in_scope(&ACME))
		.await
	}
	.unwrap();
	assert_eq!(run.phase().as_str(), "WAITING");
	let request_id = request_id(&run);
	for child in children {
		assert!(
			store
				.abandon_task(&ACME, child.id, child.revision + 1, "No longer needed")
				.await
				.is_err()
		);
		assert!(
			store
				.abandon_task(&ACME, child.id, child.revision, " ")
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
				json!({"revision":child.revision,"reason":"Deployment Administrator accepts partial results"})
					.to_string(),
			)
			.send()
			.await
			.unwrap();
		assert_eq!(response.status(), StatusCode::OK);
		assert_eq!(
			store.task(&ACME, child.id).await.unwrap().status.as_str(),
			"ABANDONED"
		);
	}
	store
		.answer(
			&ACME,
			request_id,
			json!("Continue with the remaining results"),
		)
		.await
		.unwrap();
	harness.worker_once().await.unwrap();
	assert_eq!(
		store.run(&ACME, run.id).await.unwrap().phase().as_str(),
		"THINKING"
	);
	final_response(&store, parent.id).await;
	harness.worker_once().await.unwrap();
	assert_eq!(
		store.task(&ACME, parent.id).await.unwrap().status.as_str(),
		"COMPLETED"
	);
	assert_eq!(
		store
			.snapshot(&ACME, workspace.id)
			.await
			.unwrap()
			.artifacts
			.len(),
		1
	);
	let events = store
		.events(&ACME, 0, Some(workspace.id), 1000)
		.await
		.unwrap();
	assert_eq!(
		events
			.iter()
			.filter(|e| e.kind == "task.abandoned"
				&& e.data["reason"] == "Deployment Administrator accepts partial results")
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
		.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry(
			"tool",
			"retry-tool",
			json!({"registry_deployment":f.config.deployment,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":endpoint,"replay":"idempotent"}}),
		))
		.await
		.unwrap();
	let agent = f.registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry("agent", "retry-agent", json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Test retries","schema_version":1,"bindings":[{"kind":"tool","target":{"registry_deployment":f.config.deployment,"id":"retry-tool","version":"1.0.0"},"alias":"plugin_0","narrow":{}}],"remove_default":[]}))).await.unwrap();
	let workspace = store
		.create_workspace(&ACME, "alice", "Retries", "Independent budgets")
		.await
		.unwrap();
	let task = running_task(&f, &agent, workspace.id, None).await;
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
		.execute(store.pool.fixture_driver())
		.await
	}
	.unwrap();
	let harness = aidash_server::harness::Harness { federation: f };
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
		.fetch_one(store.pool.in_scope(&ACME))
		.await
	}
	.unwrap();
	assert_eq!(json!(run.state)["data"]["cursor"], 1);
	assert!(run.recovery.retry.is_none());
	assert!(run.error.is_none());
	harness.worker_once().await.unwrap();
	let run = store.run(&ACME, run.id).await.unwrap();
	assert_eq!(run.phase().as_str(), "TOOL_CALL");
	assert_eq!(run.recovery.retry.as_ref().unwrap().count, 1);
	assert_eq!(
		store.task(&ACME, task.id).await.unwrap().status.as_str(),
		"RUNNING"
	);
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
	// The model fixture is a Keyless endpoint on loopback (ADR 0036).
	federation
		.store
		.keyless
		.permit_origin_of(&endpoint)
		.unwrap();
	federation.registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry("model", "web-model", json!({"provider":"openrouter","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}))).await.unwrap();
	federation
		.registry
		.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), { let mut tool = entry(
			"tool",
			"web-fetch",
			json!({"registry_deployment":federation.config.deployment,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":format!("{endpoint}/evidence"),"replay":"read_only"}}),
		); tool.schema = json!({"type":"object","properties":{"url":{"type":"string","pattern":"^http://127\\.0\\.0\\.1:"}},"required":["url"],"additionalProperties":false}); tool })
		.await
		.unwrap();
	let agent = federation.registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry("agent", "web-research", json!({"model":{"id":"web-model","version":"1.0.0"},"instructions":"Use available public evidence","schema_version":1,"bindings":[{"kind":"tool","target":{"registry_deployment":federation.config.deployment,"id":"web-fetch","version":"1.0.0"},"alias":"plugin_0","narrow":{}}],"remove_default":[]}))).await.unwrap();
	let workspace = store
		.create_workspace(&ACME, "alice", "Web research", "Use available evidence")
		.await
		.unwrap();
	let task = running_task(&federation, &agent, workspace.id, None).await;
	let harness = Harness { federation };
	for _ in 0..40 {
		let progress = harness.worker_once().await.unwrap();
		if !progress {
			break;
		}
		if store.task(&ACME, task.id).await.unwrap().status
			== aidash_server::domain::TaskStatus::Completed
		{
			break;
		}
	}
	let run = store.runs(&ACME).await.unwrap().remove(0);
	assert_eq!(
		run.phase().as_str(),
		"COMPLETED",
		"error={:?}, pending={}, history={}",
		run.error,
		json!(run.state)["data"],
		json!(run.context.history)
	);
	assert_eq!(
		store.task(&ACME, task.id).await.unwrap().status.as_str(),
		"COMPLETED"
	);
	assert_eq!(
		run.context
			.events()
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
		store.snapshot(&ACME, workspace.id).await.unwrap().artifacts[0].content,
		json!("Used alternate evidence")
	);
	assert!(
		!store
			.events(&ACME, 0, Some(workspace.id), 100)
			.await
			.unwrap()
			.iter()
			.any(|event| event.kind == "run.retrying")
	);
	drop(server);
	cleanup(store, &database_url, &schema).await;
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
		.create_workspace(&ACME, "alice", "Children", "Atomic completion")
		.await
		.unwrap();
	let parent = running_task(&f, &agent, workspace.id, None).await;
	let mut input = new_task();
	input.parent_id = Some(parent.id);
	let artifact = ArtifactInput {
		kind: "text".into(),
		name: "parent".into(),
		content: json!("done"),
	};
	let (child, completed) = tokio::join!(
		store.create_task(&ACME, workspace.id, &input, "human", Some("child")),
		store.complete(
			&ACME,
			parent.id,
			parent.owner.as_deref().unwrap(),
			"parent",
			&artifact
		)
	);
	assert_ne!(child.is_ok(), completed.is_ok());
	if let Ok(child) = child {
		assert_eq!(
			store.task(&ACME, parent.id).await.unwrap().status.as_str(),
			"RUNNING"
		);
		let cancelled = store
			.transition(
				&ACME,
				child.id,
				child.revision,
				"human",
				aidash_server::domain::TaskStatus::Cancelled,
			)
			.await
			.unwrap();
		store
			.abandon_task(&ACME, child.id, cancelled.revision, "No longer required")
			.await
			.unwrap();
		store
			.complete(
				&ACME,
				parent.id,
				parent.owner.as_deref().unwrap(),
				"parent",
				&artifact,
			)
			.await
			.unwrap();
		assert_eq!(
			store
				.create_task(&ACME, workspace.id, &input, "human", Some("child"))
				.await
				.unwrap()
				.id,
			child.id
		);
	}
	assert!(
		store
			.create_task(&ACME, workspace.id, &input, "human", Some("new-child"))
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
		.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry(
			"skill",
			"bundled-skill",
			json!({"instructions":"Read references/guide.md","files":[{"path":"references/guide.md","content":"界".repeat(3000)}]}),
		))
		.await
		.unwrap();
	let mut agent = seed(&f.registry).await;
	agent.id = "skilled-agent".into();
	agent.binding_normalization = None;
	agent.config["bindings"] = json!([{"kind":"skill","target":{"registry_deployment":f.config.deployment,"id":"bundled-skill","version":"1.0.0"},"narrow":{}}]);
	f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			agent.clone(),
		)
		.await
		.unwrap();
	let workspace = store
		.create_workspace(&ACME, "alice", "Skill read", "Read a bundled guide")
		.await
		.unwrap();
	let task = running_task(&f, &agent, workspace.id, None).await;
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
		.execute(store.pool.fixture_driver())
		.await
	}
	.unwrap();
	aidash_server::harness::Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let run = store.runs(&ACME).await.unwrap().remove(0);
	let aidash_server::context::ContextEvent::Tool { result, .. } = &run.context.history[0].event
	else {
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
		.create_workspace(&ACME, "alice", "Recovery", "Repair model errors")
		.await
		.unwrap();
	let parent = running_task(&f, &agent, workspace.id, None).await;
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
		.execute(store.pool.fixture_driver())
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
		.fetch_one(store.pool.in_scope(&ACME))
		.await
	}
	.unwrap();
	assert_eq!(json!(run.state)["data"]["cursor"], 1);
	assert!(
		json!(run.context)["history"][0]["event"]["result"]["error"]
			.as_str()
			.unwrap()
			.contains("missing_tool")
	);
	let mut input = new_task();
	input.parent_id = Some(parent.id);
	let child = store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	final_response(&store, parent.id).await;
	harness.worker_once().await.unwrap();
	let waiting = store.run(&ACME, run.id).await.unwrap();
	assert_eq!(waiting.phase().as_str(), "WAITING");
	assert_eq!(waiting.step, run.step + 1);
	let child = store
		.transition(
			&ACME,
			child.id,
			child.revision,
			"human",
			aidash_server::domain::TaskStatus::Cancelled,
		)
		.await
		.unwrap();
	store
		.abandon_task(&ACME, child.id, child.revision, "No longer required")
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
		.execute(store.pool.fixture_driver())
		.await
	}
	.unwrap();
	harness.worker_once().await.unwrap();
	assert_eq!(
		store.task(&ACME, parent.id).await.unwrap().status.as_str(),
		"COMPLETED"
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_agent_runs_only_on_a_node_that_approved_it(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	common::approve_agent(&f, "acme", &agent).await;
	let first = common::default_node(&f.store, "acme").await;
	let second_name = common::second_node_in(f.store.pool.fixture_driver(), "acme").await;
	let second = f.store.deployment.node(second_name.clone());
	assert_ne!(first, second);
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Build", "Run on another Node")
		.await
		.unwrap();
	sqlx::query("UPDATE workspaces SET node = $1 WHERE id = $2")
		.bind(second_name.as_str())
		.bind(workspace.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let task = f
		.store
		.create_task(&ACME, workspace.id, &new_task(), "human", None)
		.await
		.unwrap();

	assert!(
		common::subject_claim(&f, "acme", &task, &agent)
			.await
			.is_err(),
		"an approval on the first Node does not stand for the second"
	);
	assert!(
		f.store
			.runs(&ACME)
			.await
			.unwrap()
			.iter()
			.all(|run| run.task_id != task.id)
	);

	common::approve_agent_on(&f, "acme", &agent, &second).await;
	common::subject_claim(&f, "acme", &task, &agent)
		.await
		.expect("the second Node approved the agent");
	let run = f
		.store
		.runs(&ACME)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.task_id == task.id)
		.expect("the claim queues a Run");
	assert_eq!(run.node, second_name);
	assert_eq!(run.home_node, second.to_string());
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
	let agent = seed(&f.registry).await;
	common::approve_agent(&f, "acme", &agent).await;
	let token = common::tenant_token(&store, "acme").await;
	let app = _application.application.clone();
	// Simulate an event insertion failure after the primary mutation.
	// SeaQuery cannot add a CHECK constraint to an existing table.
	// Only the table owner can add a constraint; the server role holds DML.
	let owner = common::owner_pool(store.pool.fixture_driver()).await;
	sqlx::query(
		"ALTER TABLE events ADD CONSTRAINT reject_registration CHECK(kind <> 'registry.registered')",
	)
	.execute(&owner)
	.await
	.unwrap();
	let response = http_client
		.request(Method::POST, app.url("/api/registry"))
		.header("authorization", format!("Bearer {token}"))
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
		Registry::new(store.pool.clone(), &store.deployment)
			.unwrap()
			.get(ACME.tenant_name().unwrap(), "rollback", "1.0.0")
			.await
			.is_err()
	);
	// SeaQuery cannot add a CHECK constraint to an existing table.
	sqlx::query(
		"ALTER TABLE events ADD CONSTRAINT reject_conversation CHECK(kind <> 'conversation.created')",
	)
	.execute(&owner)
	.await
	.unwrap();
	let response=http_client.request(Method::POST, app.url("/api/conversations"))
		.header("authorization", format!("Bearer {token}"))
		.header("content-type","application/json")
		.body(json!({"title":"Atomic","goal":"Must roll back","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}).to_string())
		.send().await.unwrap();
	assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
	assert!(store.workspaces(&ACME).await.unwrap().is_empty());
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
	let bus = aidash_server::bus::EventBus::connect(&f.config.nats_url, store.deployment.as_str())
		.await
		.unwrap();
	let poison = store
		.emit(&ACME, None, "large", json!({"text":"x".repeat(2_000_000)}))
		.await
		.unwrap();
	let healthy = store
		.emit(&ACME, None, "small", json!({"ok":true}))
		.await
		.unwrap();
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
		.fetch_one(store.pool.fixture_driver())
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
			let replay = store
				.events(&ACME, poison.sequence - 1, None, 500)
				.await
				.unwrap();
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
			.fetch_one(store.pool.fixture_driver())
			.await
		}
		.unwrap()
	);
	bus.context.delete_stream(&bus.stream_name).await.unwrap();
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
		.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), entry(
			"tool",
			"unsafe-http",
			json!({"registry_deployment":f.config.deployment,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"http://127.0.0.1:9","replay":"unsafe"}}),
		))
		.await
		.unwrap();
	agent.id = "unsafe-agent".into();
	agent.binding_normalization = None;
	agent.config["bindings"] = json!([{"kind":"tool","target":{"registry_deployment":f.config.deployment,"id":"unsafe-http","version":"1.0.0"},"alias":"plugin_0","narrow":{}}]);
	f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			agent.clone(),
		)
		.await
		.unwrap();
	let workspace = store
		.create_workspace(
			&ACME,
			"alice",
			"Reconcile",
			"Never expose an incomplete request",
		)
		.await
		.unwrap();
	let task = running_task(&f, &agent, workspace.id, None).await;
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
		.execute(store.pool.fixture_driver())
		.await
	}
	.unwrap();
	let worker = Uuid::new_v4();
	let run = store.lease_run(worker, 30).await.unwrap().unwrap();
	let key = format!("{}:0:0", run.id);
	store
		.invocation_start(&ACME, &run, worker, &key, "plugin_0", &json!({}), false)
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
		.execute(store.pool.fixture_driver())
		.await
	}
	.unwrap();
	aidash_server::harness::Harness { federation: f }
		.worker_once()
		.await
		.unwrap();
	let current = store.run(&ACME, run.id).await.unwrap();
	let request = request_id(&current);
	assert_eq!(json!(current.state)["data"]["key"], key);
	assert!(matches!(
		store.answer(&ACME, request, json!("done")).await,
		Err(aidash_server::Error::Invalid(_))
	));
	store
		.answer(&ACME, request, json!({"result":"verified"}))
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
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	for coordinator in ["missing", "model"] {
		assert!(
			f.registry
				.register(
					&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
					entry(
						"cluster",
						coordinator,
						json!({"coordinator":{"id":coordinator,"version":"1.0.0"}})
					)
				)
				.await
				.is_err()
		);
	}
	f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			entry(
				"cluster",
				"valid-cluster",
				json!({"coordinator":{"id":"research","version":"1.0.0"}}),
			),
		)
		.await
		.unwrap();
	let acme = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	f.registry
		.register(
			&acme,
			entry(
				"tool",
				"delegate-tool",
				json!({"registry_deployment":f.config.deployment,"provider":"integration.agent@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","narrow":{},"transport":{"transport":"agent","node_id":f.config.deployment,"agent":{"id":agent.id,"version":agent.version}}}),
			),
		)
		.await
		.unwrap();
	let delegating = f
		.registry
		.register(
			&acme,
			entry(
				"agent",
				"delegating",
				json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Delegate","schema_version":1,"bindings":[{"kind":"tool","target":{"registry_deployment":f.config.deployment,"id":"delegate-tool","version":"1.0.0"},"alias":"plugin_0","narrow":{}}],"remove_default":[]}),
			),
		)
		.await
		.unwrap();
	common::approve_agent(&f, "acme", &agent).await;
	let workspace = store
		.create_workspace(&ACME, "alice", "Agent tool", "Await delegated results")
		.await
		.unwrap();
	let task = running_task(&f, &delegating, workspace.id, None).await;
	let response = aidash_server::provider::ModelResponse {
		tool_calls: [
			json!({"title":"Child","description":"Delegated work"}),
			json!({"title":"Child","description":"Delegated work","parent_id":null}),
		]
		.into_iter()
		.enumerate()
		.map(|(index, arguments)| aidash_server::provider::ToolCall {
			id: index.to_string(),
			name: "plugin_0".into(),
			arguments,
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
		.execute(store.pool.fixture_driver())
		.await
	}
	.unwrap();
	let harness = aidash_server::harness::Harness { federation: f };
	let children = async || -> Vec<Option<Uuid>> {
		let query_bind_1 = workspace.id;
		let query_bind_2 = task.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.column(reinhardt::query::Alias::new("parent_id"))
				.from(reinhardt::query::Alias::new("tasks"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ? AND id <> ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(store.pool.fixture_driver())
		.await
		.unwrap()
	};
	for _ in 0..6 {
		if children().await.len() == 2 {
			break;
		}
		harness.worker_once().await.unwrap();
	}
	assert_eq!(children().await, vec![Some(task.id); 2]);
	let delegated: i64 = {
		let query_bind_1 = task.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from_as(reinhardt::query::Alias::new("runs"), reinhardt::query::Alias::new("r"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(r.task_id IN (SELECT id FROM tasks WHERE parent_id = ?) AND r.agent_id = 'research')"
						.to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(delegated, 2, "each child is delegated to the bound agent");
	cleanup(store, &url, &schema).await;
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
		runtime.federation.registry = Registry::new(store.pool.clone(), &store.deployment).unwrap();
		runtime.federation.config = Config {
			deployment: store.deployment.clone(),
			endpoint: "http://127.0.0.1:18080".into(),
			database_url: runtime.url.clone(),
			nats_url: "nats://127.0.0.1:42270".into(),
			api_token: "test-access-token".into(),
			web_dir: "web/dist".into(),
			lease_seconds: 30,
			tenancy: aidash_domain::identity::tenancy::TenancyRules::of(
				aidash_domain::offering::Offering::SelfHosted,
			),
			demo_data: false,
			oidc: None,
			sign_in: None,
			federation: true,
			deployment_key: None,
			tier_limits: std::sync::Arc::new(aidash_application::ports::nodes::NoTierLimits),
		};
		runtime.federation.client = http_client;
		runtime
	}
	.boxed()
	.shared()
}
#[path = "postgres/inference_usage.rs"]
mod inference_usage;

#[path = "postgres/research.rs"]
mod research;

#[rstest::fixture]
fn outbox_store(#[from(common::runtime)] runtime: common::RuntimeFuture) -> StoreFuture {
	let node = format!("aidash://outbox-{}", Uuid::new_v4().simple());
	store(&node, runtime)
}
