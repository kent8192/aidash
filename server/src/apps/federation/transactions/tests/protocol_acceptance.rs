#[path = "../../../execution/tests/support/legacy.rs"]
mod common;
#[path = "../../../execution/tests/support/worker_process.rs"]
mod worker_process;
use aidash_server::{
	Error,
	apps::federation::transactions::{
		models::{coordinator_records, states::AtomicCoordinatorDecision},
		services::decisions::CoordinatorTransition,
	},
	federation::{Federation, Peer},
	registry::Registry,
	transactions::{Manifest, coordinator, participant},
};
use chrono::{Duration, Utc};
use common::{TestEnvironment, test_environment};
use reinhardt::db::backends::{DatabaseConnection, dialect::PostgresBackend};
use reinhardt::db::orm::DatabaseConnectionLease;
use reinhardt::test::fixtures::http_client;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;
use worker_process::WorkerProcess;

use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use rstest::fixture;
type EnvironmentFuture = Shared<BoxFuture<'static, Arc<TestEnvironment>>>;
type Pair = (Node, Node, Manifest, Uuid, Uuid);
type PairFuture = BoxFuture<'static, Pair>;

#[fixture]
fn atomic_runtime(
	#[default("a")] suffix: &str,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	let suffix = suffix.to_owned();
	async move {
		let mut runtime = runtime.await;
		let f = &mut runtime.federation;
		f.config.node_id = format!("aidash://atomic-{suffix}");
		f.store.node_id = f.config.node_id.clone();
		f.config.api_token = "atomic-operator-fixture-token".into();
		f.registry = Registry::new(f.store.pool.clone(), &f.config.node_id).unwrap();
		runtime
	}
	.boxed()
	.shared()
}
#[fixture]
fn node(
	#[default("a")] _suffix: &str,
	#[from(test_environment)] _environment: EnvironmentFuture,
	#[default(aidash_server::sse::Service::new(Default::default()))]
	_streams: aidash_server::sse::Service,
	#[from(common::execution_database)]
	#[with(_environment.clone(), &format!("atomic_{}",_suffix.replace('-',"_")), 10)]
	_database: common::DatabaseFuture,
	#[from(common::runtime)]
	#[with(_database.clone())]
	_runtime: common::RuntimeFuture,
	#[from(atomic_runtime)]
	#[with(_suffix,_runtime.clone())]
	_atomic: common::RuntimeFuture,
	#[from(common::native_peer)]
	#[with(&format!("aidash://atomic-{_suffix}"),Arc::new(|router|router),_atomic.clone(),_streams.clone())]
	peer: common::PeerFuture,
) -> BoxFuture<'static, Node> {
	async move {
		let peer = peer.await;
		let f = peer.runtime.federation.clone();
		let listen = f
			.config
			.endpoint
			.strip_prefix("http://")
			.unwrap()
			.parse()
			.unwrap();
		Node {
			f,
			listen,
			server: Some(peer.server),
			client: peer.client,
			application: peer.application,
			_runtime: peer.runtime,
			_capacity: None,
		}
	}
	.boxed()
}
#[fixture]
fn pair_capacity() -> BoxFuture<'static, tokio::sync::OwnedSemaphorePermit> {
	static CAPACITY: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
		std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));
	async { CAPACITY.clone().acquire_owned().await.unwrap() }.boxed()
}
#[fixture]
fn pair(
	#[from(test_environment)] _environment: EnvironmentFuture,
	#[default(aidash_server::sse::Service::new(Default::default()))]
	_streams: aidash_server::sse::Service,
	pair_capacity: BoxFuture<'static, tokio::sync::OwnedSemaphorePermit>,
	#[from(node)]
	#[with("a",_environment.clone(),_streams.clone())]
	first: BoxFuture<'static, Node>,
	#[from(node)]
	#[with("b",_environment.clone())]
	second: BoxFuture<'static, Node>,
) -> PairFuture {
	let capacity = Box::pin(pair_capacity);
	async move {let capacity=capacity.await; let mut a=first.await; a._capacity=Some(capacity); let b=second.await;
	for (local, remote) in [(&a, &b), (&b, &a)] {
		local
			.f
			.register_peer(Peer {
				node_id: remote.f.config.node_id.clone(),
				endpoint: remote.f.config.endpoint.clone(),
				credential_env: "AIDASH_SECRET_TEST_PEER".into(),
				protocol_version: "0.2".into(),
				enabled: true,
			})
			.await
			.unwrap();
		assert_eq!(
			local
				.request(
					reqwest::Method::POST,
					"/api/transactions/trust",
					Some(json!({"node_id":remote.f.config.node_id,"enabled":true}))
				)
				.await
				.0,
			200
		);
	}
	let wa =
		a.f.store
			.create_workspace("A", "Atomic state")
			.await
			.unwrap();
	let wb =
		b.f.store
			.create_workspace("B", "Atomic state")
			.await
			.unwrap();
	let manifest=serde_json::from_value(json!({"id":Uuid::new_v4(),"coordinator":a.f.config.node_id,"isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),"participants":[{"node_id":a.f.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":wa.id,"expected_revision":0,"state":{"value":"new-a"}}]},{"node_id":b.f.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":wb.id,"expected_revision":0,"state":{"value":"new-b"}}]}]})).unwrap();
	(a, b, manifest, wa.id, wb.id)
 }.boxed()
}
struct Node {
	f: Federation,
	server: Option<Arc<common::PeerServerGuard>>,
	client: Arc<reinhardt::test::APIClient>,
	listen: std::net::SocketAddr,
	application: common::TestApplication,
	_runtime: common::RuntimeFixture,
	_capacity: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Node {
	async fn stop(&mut self) {
		if let Some(server) = self.server.take() {
			server.shutdown().await;
		}
	}
	async fn restart(&mut self) {
		// Act: close and reconnect the participant pools, then rebind its stable origin.
		self.stop().await;
		self.f.store.pool.close().await;
		self.f.store.control_pool.close().await;
		let store = common::native_store(&self.f.config.database_url, &self.f.config.node_id).await;
		self.f = Federation {
			registry: Registry::new(store.pool.clone(), &store.node_id).unwrap(),
			store,
			notify: Arc::new(tokio::sync::Notify::new()),
			..self.f.clone()
		};
		self.application = common::application(self.f.clone()).await;
		self.serve(self.application.native_router()).await;
	}
	async fn serve(&mut self, router: Arc<reinhardt::ServerRouter>) {
		// Act: replace the live participant routes while preserving its advertised port.
		self.stop().await;
		let listener = Arc::new(tokio::net::TcpListener::bind(self.listen).await.unwrap());
		self.server = Some(Arc::new(common::PeerServerGuard::spawn(
			listener, router, None,
		)));
	}
	async fn request(
		&self,
		method: reqwest::Method,
		path: &str,
		body: Option<Value>,
	) -> (u16, Value) {
		let authorization = format!("Bearer {}", self.f.config.api_token);
		let headers = [("Authorization", authorization.as_str())];
		let client = self.client.clone();
		let response = match method {
			reqwest::Method::GET => client.get_with_headers(path, &headers).await.unwrap(),
			reqwest::Method::POST => client
				.post_raw_with_headers(
					path,
					&serde_json::to_vec(&body.unwrap_or(Value::Null)).unwrap(),
					"application/json",
					&headers,
				)
				.await
				.unwrap(),
			_ => {
				// reinhardt-web#6661: the native generic request dispatcher is private.
				let mut request = self
					.f
					.client
					.request(method, format!("{}{path}", self.f.config.endpoint))
					.bearer_auth(&self.f.config.api_token)
					.timeout(std::time::Duration::from_secs(5));
				if let Some(body) = body {
					request = request.json(&body);
				}
				let response = request.send().await.unwrap();
				let status = response.status();
				let headers = response.headers().clone();
				let version = response.version();
				let body = response.bytes().await.unwrap();
				reinhardt::test::TestResponse::with_body_and_version(status, headers, body, version)
			}
		};
		(
			response.status_code(),
			response.json_value().unwrap_or(Value::Null),
		)
	}
	async fn get(&self, path: &str) -> (u16, Value) {
		self.request(reqwest::Method::GET, path, None).await
	}
	async fn cleanup(mut self) {
		self.stop().await;
		self.f.store.pool.close().await;
		self.f.store.control_pool.close().await;
	}
}
impl Drop for Node {
	fn drop(&mut self) {
		if let Some(server) = &self.server {
			server.abort();
		}
	}
}
async fn steps(node: &Node, id: Uuid, count: usize) {
	for _ in 0..count {
		coordinator::advance(&node.f, id).await.unwrap();
	}
}
async fn complete(node: &Node, id: Uuid) -> aidash_server::transactions::Status {
	for _ in 0..40 {
		let state = coordinator::advance(&node.f, id).await.unwrap();
		if state.complete {
			return state;
		}
	}
	panic!(
		"transaction did not finish: {:?}",
		coordinator::status(&node.f, id).await.unwrap()
	);
}
async fn unavailable(node: &Node, workspace: Uuid) {
	for path in [
		"/api/state".to_string(),
		"/api/registry".into(),
		format!("/api/workspaces/{workspace}"),
		"/api/events".into(),
		"/.well-known/aidash".into(),
	] {
		assert_eq!(node.get(&path).await.0, 503, "{path}");
	}
	assert_eq!(node.get("/health").await.0, 200);
	let (session_status, session) = node.get("/api/session").await;
	assert_eq!(session_status, 200);
	assert_eq!(session["access"]["kind"], "operator");
	assert_eq!(node.get("/api/transactions/participants").await.0, 200);
	assert!(matches!(
		aidash_server::harness::Harness {
			federation: node.f.clone()
		}
		.worker_once()
		.await,
		Err(Error::TransactionPending)
	));
	assert!(matches!(
		aidash_server::generation::provision::reconcile(&node.f).await,
		Err(Error::TransactionPending)
	));
	assert!(
		node.f
			.store
			.update_state(workspace, 0, json!({"bypass":true}))
			.await
			.is_err(),
		"statement trigger must reject a direct write"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn abort_records_a_durable_decision_while_recovery_owns_the_transition_lease(
	#[future(awt)] pair: Pair,
) {
	let (a, b, manifest, wa, wb) = pair;
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 4).await; // Both votes are prepared, still undecided.
	let mut transition = a.f.store.control_pool.driver().begin().await.unwrap();
	{
		let query_bind_1 = manifest.id.to_string();
		sqlx::query(
			&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED('atomic:' || ?, 0)))".to_owned(),
					vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut *transition)
		.await
	}
	.unwrap();
	// A recovery step can hold this lease during slow peer I/O. An operator's
	// durable abort must not be lost merely because that step is in flight.
	let (status, body) = a
		.request(
			reqwest::Method::POST,
			&format!("/api/transactions/{}/abort", manifest.id),
			None,
		)
		.await;
	transition.commit().await.unwrap();
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["decision"], "ABORT");
	assert!(!body["complete"].as_bool().unwrap());
	assert_eq!(
		coordinator::abort(&a.f, manifest.id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("ABORT")
	);
	let final_state = complete(&a, manifest.id).await;
	assert_eq!(final_state.decision.as_deref(), Some("ABORT"));
	assert_eq!(a.f.store.workspace(wa).await.unwrap().state, json!({}));
	assert_eq!(b.f.store.workspace(wb).await.unwrap().state, json!({}));
	let decisions: i64 = { let query_bind_1 = manifest.id; sqlx::query_scalar(&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("atomic_history"))
			.and_where(SimpleExpr::CustomWithExpr("(transaction_id = ? AND role = 'coordinator' AND phase IN ('COMMIT', 'ABORT'))".to_owned(), vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()]))
			.to_string(reinhardt::query::PostgresQueryBuilder))
	.fetch_one(a.f.store.control_pool.driver())
	.await }
	.unwrap();
	assert_eq!(decisions, 1);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn two_node_commit_hides_partial_application_and_releases_only_after_all_apply(
	#[future(awt)] pair: Pair,
) {
	let (a, b, manifest, wa, wb) = pair;
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 2).await;
	unavailable(&a, wa).await;
	unavailable(&b, wb).await;
	assert!(matches!(
		a.f.request::<Value>(
			&b.f.config.node_id,
			reqwest::Method::POST,
			"/discover",
			Some(&json!({}))
		)
		.await,
		Err(Error::TransactionPending)
	));
	steps(&a, manifest.id, 2).await; // Both prepared; speculative writes rolled back.
	assert_eq!(a.f.store.workspace(wa).await.unwrap().state, json!({}));
	assert_eq!(b.f.store.workspace(wb).await.unwrap().state, json!({}));
	steps(&a, manifest.id, 1).await; // Durable commit precedes any application.
	assert_eq!(
		coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("COMMIT")
	);
	assert!(coordinator::abort(&a.f, manifest.id).await.is_err());
	let connection = DatabaseConnectionLease::register(DatabaseConnection::new(Arc::new(
		PostgresBackend::new(a.f.store.control_pool.driver().clone()),
	)))
	.unwrap();
	assert!(
		!coordinator_records::transition(
			connection.handle(),
			manifest.id,
			CoordinatorTransition::Decide(AtomicCoordinatorDecision::Abort),
			"late conflicting decision",
		)
		.await
		.unwrap()
	);
	assert_eq!(
		coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("COMMIT")
	);
	steps(&a, manifest.id, 1).await;
	assert_eq!(
		a.f.store.workspace(wa).await.unwrap().state["value"],
		"new-a"
	);
	assert_eq!(b.f.store.workspace(wb).await.unwrap().state, json!({}));
	unavailable(&a, wa).await;
	unavailable(&b, wb).await;
	steps(&a, manifest.id, 1).await;
	assert_eq!(
		b.f.store.workspace(wb).await.unwrap().state["value"],
		"new-b"
	);
	assert!(
		!coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.visible
	);
	steps(&a, manifest.id, 1).await; // Visibility certificate, barriers still retained.
	assert!(
		coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.visible
	);
	steps(&a, manifest.id, 1).await;
	assert_eq!(a.get(&format!("/api/workspaces/{wa}")).await.0, 200);
	assert_eq!(b.get(&format!("/api/workspaces/{wb}")).await.0, 503);
	let done = complete(&a, manifest.id).await;
	assert!(done.visible);
	for (node, workspace) in [(&a, wa), (&b, wb)] {
		assert_eq!(
			node.get(&format!("/api/workspaces/{workspace}")).await.0,
			200
		);
		assert_eq!(
			participant::finish(&node.f, &a.f.config.node_id, &manifest)
				.await
				.unwrap()
				.phase,
			"COMMITTED"
		);
		let updates: i64 = {
			let query_bind_1 = workspace;
			sqlx::query_scalar(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::cust("COUNT(*)"))
					.from(reinhardt::query::Alias::new("events"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(workspace_id = ? AND kind = 'workspace.updated')".to_owned(),
						vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(node.f.store.pool.driver())
			.await
		}
		.unwrap();
		assert_eq!(updates, 1);
	}
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn stale_prepare_aborts_every_node_and_delayed_reserve_cannot_resurrect_it(
	#[future(awt)] pair: Pair,
) {
	let (a, b, mut manifest, wa, wb) = pair;
	if let aidash_server::transactions::Mutation::WorkspaceState {
		expected_revision, ..
	} = &mut manifest.participants[1].mutations[0]
	{
		*expected_revision = 9;
	}
	coordinator::submit(&a.f, &manifest).await.unwrap();
	let done = complete(&a, manifest.id).await;
	assert_eq!(done.decision.as_deref(), Some("ABORT"));
	assert!(!done.visible);
	for (node, workspace) in [(&a, wa), (&b, wb)] {
		assert_eq!(
			node.f.store.workspace(workspace).await.unwrap().state,
			json!({})
		);
		assert_eq!(
			node.get(&format!("/api/workspaces/{workspace}")).await.0,
			200
		);
		assert_eq!(
			participant::reserve(&node.f, &a.f.config.node_id, &manifest)
				.await
				.unwrap()
				.phase,
			"ABORTED"
		);
		assert_eq!(
			participant::finish(&node.f, &a.f.config.node_id, &manifest)
				.await
				.unwrap()
				.phase,
			"ABORTED"
		);
	}
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn partition_after_commit_retains_barriers_and_restart_recovers_the_same_decision(
	#[future(awt)] pair: Pair,
) {
	let (mut a, mut b, manifest, wa, wb) = pair;
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 5).await;
	b.stop().await;
	steps(&a, manifest.id, 2).await;
	let waiting = coordinator::status(&a.f, manifest.id).await.unwrap();
	assert_eq!(waiting.decision.as_deref(), Some("COMMIT"));
	assert!(!waiting.visible);
	assert!(waiting.last_error.is_some());
	unavailable(&a, wa).await;
	a.restart().await;
	b.restart().await;
	unavailable(&a, wa).await;
	unavailable(&b, wb).await;
	let done = complete(&a, manifest.id).await;
	assert_eq!(done.decision.as_deref(), Some("COMMIT"));
	assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
	assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn overlapping_coordinators_use_the_same_node_order_without_lost_updates(
	#[future(awt)] pair: Pair,
) {
	let (a, b, first, wa, wb) = pair;
	let mut second = first.clone();
	second.id = Uuid::new_v4();
	second.coordinator = b.f.config.node_id.clone();
	coordinator::submit(&a.f, &first).await.unwrap();
	coordinator::submit(&b.f, &second).await.unwrap();
	steps(&a, first.id, 1).await;
	steps(&b, second.id, 1).await;
	let waiting = coordinator::status(&b.f, second.id).await.unwrap();
	assert!(waiting.decision.is_none());
	assert!(waiting.last_error.is_some());
	assert_eq!(
		complete(&a, first.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	assert_eq!(
		complete(&b, second.id).await.decision.as_deref(),
		Some("ABORT")
	);
	assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
	assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn registry_workspace_task_execution_and_artifact_commit_together_once(
	#[future(awt)] pair: Pair,
) {
	use aidash_server::{
		domain::{NewTask, qualified_agent},
		registry::Entry,
	};
	let (a, b, mut manifest, wa, _wb) = pair;
	let agent:Entry=serde_json::from_value(json!({"id":"executor","version":"1.0.0","kind":"agent","name":{"en":"Executor"},"description":{"en":"Atomic fixture"},"config":{"model":{"id":"fixture","version":"1.0.0"},"instructions":"Atomic execution","schema_version":1,"bindings":[],"remove_default":[]}})).unwrap();
	let model: Entry = serde_json::from_value(json!({"id":"fixture","version":"1.0.0","kind":"model","name":{"en":"Atomic model"},"description":{"en":"Pinned execution fixture"},"config":{"provider":"openrouter","model_id":"fixture","endpoint":"http://localhost:19999/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}})).unwrap();
	b.f.registry.register(model).await.unwrap();
	b.f.registry.register(agent.clone()).await.unwrap();

	let owner = qualified_agent(&b.f.config.node_id, &agent.id, &agent.version);
	let task =
		a.f.store
			.create_task(
				wa,
				&NewTask {
					title: "Atomic task".into(),
					description: "Finish with the remote execution".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"operator",
				None,
			)
			.await
			.unwrap();
	a.f.store.claim(task.id, 0, &owner, &agent).await.unwrap();
	let task =
		a.f.store
			.transition(
				task.id,
				1,
				&owner,
				aidash_server::domain::TaskStatus::Running,
			)
			.await
			.unwrap();
	{
		let query_bind_1 = task.id;
		let query_bind_2 = &b.f.config.node_id;
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
							vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![reinhardt::query::Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![reinhardt::query::Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![reinhardt::query::Expr::value(query_bind_4.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("TRUE"))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(a.f.store.pool.driver())
		.await
	}
	.unwrap();
	let run =
		b.f.store
			.accept_run(&task, &a.f.config.node_id, &agent.id, &agent.version)
			.await
			.unwrap();
	{ let query_bind_1 = run.id; let query_bind_2 = common::tool_pending(json!({"response":{"text":"one committed result","tool_calls":[],"input_tokens":0,"output_tokens":0},"cursor":0})); sqlx::query(&reinhardt::query::Query::update().table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("phase"), reinhardt::query::Expr::cust("'TOOL_CALL'")).value_expr(reinhardt::query::Alias::new("pending"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![reinhardt::query::Expr::value(query_bind_2.to_owned()).into()])).and_where(SimpleExpr::CustomWithExpr("(id = ?)".to_owned(), vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder))
        .execute(b.f.store.pool.driver())
        .await }
        .unwrap();
	manifest.participants[0].mutations.push(serde_json::from_value(json!({"kind":"registry_register","entry":{"id":"atomic-skill","version":"1.0.0","kind":"skill","name":{"en":"Atomic skill"},"description":{"en":"Prepared registration"},"config":{"instructions":"Complete atomic work"}}})).unwrap());
	manifest.participants[0].mutations.push(serde_json::from_value(json!({"kind":"complete_task","task_id":task.id,"expected_revision":task.revision,"artifact":{"kind":"text","name":"Atomic result","content":"one committed result"}})).unwrap());
	manifest.participants[1].mutations.push(
		serde_json::from_value(
			json!({"kind":"finish_run","run_id":run.id,"task_id":task.id,"expected_revision":run.revision}),
		)
		.unwrap(),
	);
	let mut invalid = manifest.clone();
	invalid.id = Uuid::new_v4();
	{
		let query_bind_1 = run.id;
		let query_bind_2 = json!([{"id":"unfinished","name":"http","arguments":{}}]);
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(JSONB_SET(pending, '{data,response,tool_calls}', ?))".to_owned(),
						vec![reinhardt::query::Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(b.f.store.pool.driver())
		.await
	}
	.unwrap();
	coordinator::submit(&a.f, &invalid).await.unwrap();
	assert_eq!(
		complete(&a, invalid.id).await.decision.as_deref(),
		Some("ABORT")
	);
	assert_eq!(
		a.f.store.task(task.id).await.unwrap().status,
		aidash_server::domain::TaskStatus::Running
	);
	assert!(a.f.store.snapshot(wa).await.unwrap().artifacts.is_empty());
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					reinhardt::query::Expr::cust(
						"JSONB_SET(pending, '{data,response,tool_calls}', CAST('[]' AS JSONB))",
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(b.f.store.pool.driver())
		.await
	}
	.unwrap();
	coordinator::submit(&a.f, &manifest).await.unwrap();
	let committed = complete(&a, manifest.id).await;
	assert_eq!(
		committed.decision.as_deref(),
		Some("COMMIT"),
		"{committed:?}"
	);
	for _ in 0..2 {
		participant::finish(&a.f, &a.f.config.node_id, &manifest)
			.await
			.unwrap();
		participant::finish(&b.f, &a.f.config.node_id, &manifest)
			.await
			.unwrap();
	}
	assert_eq!(
		a.f.store.task(task.id).await.unwrap().status,
		aidash_server::domain::TaskStatus::Completed
	);
	assert_eq!(
		b.f.store.run(run.id).await.unwrap().phase().as_str(),
		"COMPLETED"
	);
	assert_eq!(
		a.f.registry
			.get("atomic-skill", "1.0.0")
			.await
			.unwrap()
			.kind,
		"skill"
	);
	let artifacts = a.f.store.snapshot(wa).await.unwrap().artifacts;
	assert_eq!(artifacts.len(), 1);
	assert_eq!(artifacts[0].created_by, owner);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn participant_pulls_only_durable_decisions_and_never_guesses_after_timeout(
	#[future(awt)] pair: Pair,
) {
	let (mut a, b, manifest, wa, wb) = pair;
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 4).await;
	a.stop().await;
	assert_eq!(participant::recover_once(&b.f).await.unwrap(), 0);
	assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 0);
	unavailable(&b, wb).await;
	a.restart().await;
	steps(&a, manifest.id, 1).await;
	// The coordinator has not sent apply, but a participant can recover its vote.
	assert_eq!(participant::recover_once(&b.f).await.unwrap(), 1);
	assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
	unavailable(&a, wa).await;
	unavailable(&b, wb).await;
	assert_eq!(
		complete(&a, manifest.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_existing_sse_stream_waits_for_atomic_visibility_before_emitting_changes(
	event_streams: aidash_server::sse::Service,
	#[from(test_environment)] _environment: EnvironmentFuture,
	#[future(awt)]
	#[from(pair)]
	#[with(_environment.clone(),event_streams.clone())]
	pair: Pair,
) {
	use futures_util::StreamExt;
	let (a, b, manifest, wa, _wb) = pair;
	// APIClient buffers bodies; the fixture-owned raw client preserves incremental SSE polling (#6661).
	let response = a
		.application
		.streaming_http
		.get(
			a.application
				.url(format!("/api/events/stream?workspace_id={wa}")),
		)
		.bearer_auth(&a.f.config.api_token)
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	let mut stream = response.bytes_stream();
	let first = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert!(String::from_utf8_lossy(&first).contains("workspace.created"));
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 6).await;
	tokio::time::timeout(std::time::Duration::from_secs(3), async {
		while event_streams.snapshot().visibility_waiters != 1 {
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("the SSE reader must observe the closed transaction visibility gate");
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(300), stream.next())
			.await
			.is_err()
	);
	complete(&a, manifest.id).await;
	let event = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert!(String::from_utf8_lossy(&event).contains("new-a"));
	assert_eq!(event_streams.snapshot().visibility_waiters, 0);
	drop(stream);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn peer_trust_denial_aborts_promptly_and_revocation_preserves_admitted_recovery(
	#[future(awt)] pair: Pair,
) {
	let (a, b, manifest, wa, wb) = pair;
	let set_trust = |enabled| json!({"node_id":a.f.config.node_id,"enabled":enabled});
	assert_eq!(
		b.request(
			reqwest::Method::POST,
			"/api/transactions/trust",
			Some(set_trust(false))
		)
		.await
		.0,
		200
	);
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 2).await;
	// A terminal peer rejection must abort immediately, without waiting for the deadline.
	assert_eq!(
		coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("ABORT")
	);
	complete(&a, manifest.id).await;
	assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 0);
	assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 0);

	assert_eq!(
		b.request(
			reqwest::Method::POST,
			"/api/transactions/trust",
			Some(set_trust(true))
		)
		.await
		.0,
		200
	);
	let mut admitted = manifest.clone();
	admitted.id = Uuid::new_v4();
	// An authenticated peer cannot claim that a different node coordinates its manifest.
	let mut forged = admitted.clone();
	forged.coordinator = b.f.config.node_id.clone();
	let response =
		a.f.client
			.post(format!(
				"{}/federation/v0.1/transactions/reserve",
				b.f.config.endpoint
			))
			.bearer_auth(std::env::var("AIDASH_SECRET_TEST_PEER").unwrap())
			.header("x-aidash-node", &a.f.config.node_id)
			.header("x-aidash-protocol", "0.2")
			.json(&forged)
			.send()
			.await
			.unwrap();
	assert_eq!(response.status(), 403);
	assert_eq!(b.get(&format!("/api/workspaces/{wb}")).await.0, 200);
	coordinator::submit(&a.f, &admitted).await.unwrap();
	steps(&a, admitted.id, 2).await;
	assert_eq!(
		b.request(
			reqwest::Method::POST,
			"/api/transactions/trust",
			Some(set_trust(false))
		)
		.await
		.0,
		200
	);
	assert_eq!(
		complete(&a, admitted.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
	assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn every_durable_transition_survives_fresh_pools_and_http_servers(#[future(awt)] pair: Pair) {
	let (mut a, mut b, manifest, wa, wb) = pair;
	coordinator::submit(&a.f, &manifest).await.unwrap();
	let mut transitions = 0;
	loop {
		a.restart().await;
		b.restart().await;
		let state = coordinator::advance(&a.f, manifest.id).await.unwrap();
		transitions += 1;
		if !state.visible {
			// No node may expose a new value before the global visibility decision.
			for (node, workspace) in [(&a, wa), (&b, wb)] {
				let (status, value) = node.get(&format!("/api/workspaces/{workspace}")).await;
				assert!(
					status == 503 || (status == 200 && value["workspace"]["revision"] == 0),
					"{status}: {value}"
				);
			}
		}
		if state.complete {
			break;
		}
		assert!(transitions < 20);
	}
	assert_eq!(transitions, 11);
	assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
	assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn undecided_deadline_aborts_without_publishing_prepared_mutations(
	#[future(awt)] pair: Pair,
) {
	let (a, b, mut manifest, wa, wb) = pair;
	manifest.deadline = Utc::now() + Duration::seconds(2);
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 4).await;
	let remaining = (manifest.deadline - Utc::now())
		.to_std()
		.unwrap_or_default();
	tokio::time::sleep(remaining + std::time::Duration::from_millis(10)).await;
	assert_eq!(
		complete(&a, manifest.id).await.decision.as_deref(),
		Some("ABORT")
	);
	assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 0);
	assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 0);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn actual_worker_sigkill_after_commit_recovers_without_replaying_effects(
	#[future(awt)] pair: Pair,
	#[from(reinhardt::test::fixtures::temp_dir)] worker_directory: tempfile::TempDir,
	#[from(reinhardt::test::fixtures::temp_dir)] restart_a_directory: tempfile::TempDir,
	#[from(reinhardt::test::fixtures::temp_dir)] restart_b_directory: tempfile::TempDir,
) {
	let (a, mut b, manifest, wa, wb) = pair;
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 5).await;
	b.stop().await;
	// Act: start recovery after the committed transaction and peer partition.
	let mut worker =
		WorkerProcess::start(&a.f, &a.f.config.database_url, "public", worker_directory);
	let reached = tokio::time::timeout(std::time::Duration::from_secs(15), async {
		loop {
			let phase: String = {
				let query_bind_1 = manifest.id;
				sqlx::query_scalar(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("phase")),
						))
						.from(reinhardt::query::Alias::new("atomic_participants"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_one(a.f.store.control_pool.driver())
				.await
			}
			.unwrap();
			if phase == "APPLIED" {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await;
	assert!(
		reached.is_ok(),
		"worker status {:?}; worker log: {}",
		worker.process.try_wait(),
		worker.log()
	);
	drop(worker); // SIGKILL the real coordinator worker, including its recovery loops.
	assert_eq!(
		coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("COMMIT")
	);
	unavailable(&a, wa).await;
	b.restart().await;
	// Act: replace killed workers using fresh declared directories.
	let mut restarted_a = WorkerProcess::start(
		&a.f,
		&a.f.config.database_url,
		"public",
		restart_a_directory,
	);
	let mut restarted_b = WorkerProcess::start(
		&b.f,
		&b.f.config.database_url,
		"public",
		restart_b_directory,
	);
	let completed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
		loop {
			if coordinator::status(&a.f, manifest.id)
				.await
				.unwrap()
				.complete
			{
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await;
	assert!(
		completed.is_ok(),
		"restarted worker status {:?}, {:?}; logs: {}\n{}",
		restarted_a.process.try_wait(),
		restarted_b.process.try_wait(),
		restarted_a.log(),
		restarted_b.log()
	);
	drop(restarted_a);
	drop(restarted_b);
	for (node, workspace) in [(&a, wa), (&b, wb)] {
		assert_eq!(node.f.store.workspace(workspace).await.unwrap().revision, 1);
		let events: i64 = {
			let query_bind_1 = workspace;
			sqlx::query_scalar(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::cust("COUNT(*)"))
					.from(reinhardt::query::Alias::new("events"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(workspace_id = ? AND kind = 'workspace.updated')".to_owned(),
						vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(node.f.store.pool.driver())
			.await
		}
		.unwrap();
		assert_eq!(events, 1);
	}
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn unreachable_aborted_transactions_cannot_starve_later_local_work(
	#[future(awt)] pair: Pair,
) {
	let (a, mut b, manifest, wa, _wb) = pair;
	for _ in 0..33 {
		let mut old = manifest.clone();
		old.id = Uuid::new_v4();
		coordinator::submit(&a.f, &old).await.unwrap();
		coordinator::abort(&a.f, old.id).await.unwrap();
		steps(&a, old.id, 1).await;
	}
	b.stop().await;
	let mut local = manifest.clone();
	local.id = Uuid::new_v4();
	local.participants.truncate(1);
	coordinator::submit(&a.f, &local).await.unwrap();
	for _ in 0..20 {
		coordinator::recover_once(&a.f).await.unwrap();
		if coordinator::status(&a.f, local.id).await.unwrap().complete {
			break;
		}
	}
	assert!(coordinator::status(&a.f, local.id).await.unwrap().complete);
	assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[case::source_before_any(0, "source", "ABORT")]
#[case::uncertain_source_admission(1, "lost_reply", "ABORT")]
#[case::source_after_one(1, "source", "ABORT")]
#[case::source_after_all(usize::MAX, "source", "COMMIT")]
#[case::receiver_before_admission(1, "receiver", "ABORT")]
#[case::receiver_after_admission(2, "receiver", "COMMIT")]
#[case::trust_before_admission(1, "trust", "ABORT")]
#[case::trust_after_admission(2, "trust", "COMMIT")]
#[case::mapping_before_admission(1, "mapping", "ABORT")]
#[case::mapping_after_admission(2, "mapping", "COMMIT")]
#[case::recipient_disclosure_denied(0, "disclosure", "REJECTED")]
#[tokio::test]
async fn mapped_transaction_admission_and_revocation(
	#[case] transitions: usize,
	#[case] revocation: &str,
	#[case] expected: &str,
	#[values(2, 3)] count: usize,

	#[future(awt)]
	#[from(mapped_nodes)]
	#[with(count)]
	nodes: (Pair, Option<Node>),
	fault_signals: FaultSignals,
	http_client: reqwest::Client,
) {
	use aidash_server::authorization::{
		Authorization,
		peer::{PeerMappingInput, write},
	};
	let (mut a, mut b, mut manifest, _, _) = nodes.0;
	let app_a = a.application.clone();
	let app_b = b.application.clone();
	let (_, token_a, task_a) = common::bootstrap(&a.f, &app_a, "http://127.0.0.1:9").await;
	let (_, _, task_b) = common::bootstrap(&b.f, &app_b, "http://127.0.0.1:9").await;
	let wa = a.f.store.task(task_a).await.unwrap().workspace_id;
	let wb = b.f.store.task(task_b).await.unwrap().workspace_id;
	let auth_a = Authorization {
		pool: a.f.store.control_pool.clone(),
	};
	let auth_b = Authorization {
		pool: b.f.store.control_pool.clone(),
	};
	let credential_a = auth_a
		.credentials("acme")
		.await
		.unwrap()
		.into_iter()
		.find(|c| c.subject == "alice")
		.unwrap();
	let credential_b = auth_b
		.credentials("acme")
		.await
		.unwrap()
		.into_iter()
		.find(|c| c.subject == "alice")
		.unwrap();
	write(
		&b.f,
		"acme",
		PeerMappingInput {
			source_node: a.f.config.node_id.clone(),
			source_tenant: "acme".into(),
			source_subject: "alice".into(),
			credential_id: credential_b.id,
			enabled: true,
			expected_revision: 0,
		},
	)
	.await
	.unwrap();
	for (index, workspace) in [wa, wb].into_iter().enumerate() {
		manifest.participants[index].mutations =
			vec![aidash_server::transactions::Mutation::WorkspaceState {
				workspace_id: workspace,
				expected_revision: 0,
				state: json!({"accepted":true}),
			}];
	}
	let mut third = None;
	let mut initial_third = nodes.1;
	if count == 3 {
		let c = initial_third.take().unwrap();
		for (local, remote) in [(&a, &c), (&c, &a)] {
			local
				.f
				.register_peer(Peer {
					node_id: remote.f.config.node_id.clone(),
					endpoint: remote.f.config.endpoint.clone(),
					credential_env: "AIDASH_SECRET_TRANSACTION_02".into(),
					protocol_version: "0.2".into(),
					enabled: true,
				})
				.await
				.unwrap();
			assert_eq!(
				local
					.request(
						reqwest::Method::POST,
						"/api/transactions/trust",
						Some(json!({"node_id":remote.f.config.node_id,"enabled":true}))
					)
					.await
					.0,
				200
			);
		}
		let app_c = c.application.clone();
		let (_, _, task) = common::bootstrap(&c.f, &app_c, "http://127.0.0.1:9").await;
		let workspace = c.f.store.task(task).await.unwrap().workspace_id;
		let auth = Authorization {
			pool: c.f.store.control_pool.clone(),
		};
		let credential = auth
			.credentials("acme")
			.await
			.unwrap()
			.into_iter()
			.find(|r| r.subject == "alice")
			.unwrap();
		write(
			&c.f,
			"acme",
			PeerMappingInput {
				source_node: a.f.config.node_id.clone(),
				source_tenant: "acme".into(),
				source_subject: "alice".into(),
				credential_id: credential.id,
				enabled: true,
				expected_revision: 0,
			},
		)
		.await
		.unwrap();
		manifest
			.participants
			.push(aidash_server::transactions::Participant {
				node_id: c.f.config.node_id.clone(),
				mutations: vec![aidash_server::transactions::Mutation::WorkspaceState {
					workspace_id: workspace,
					expected_revision: 0,
					state: json!({"accepted":true}),
				}],
			});
		third = Some((c, workspace));
	}
	if revocation == "disclosure" {
		let snapshot = auth_b.snapshot("acme").await.unwrap();
		let mut policy = json!(snapshot.bundle);
		policy["policies"].as_array_mut().unwrap().push(json!({"id":"no-disclosure","effect":"deny","subjects":{"any":true},"actions":["transaction.disclose"],"resources":{"kinds":["*"]}}));
		auth_b
			.replace(
				"acme",
				snapshot.revision,
				serde_json::from_value(policy).unwrap(),
				"operator",
			)
			.await
			.unwrap();
	}
	let (status, body) = common::request(
		&app_a,
		&token_a,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	if revocation == "disclosure" {
		assert_eq!(status, 403, "{body}");
		assert!(matches!(
			coordinator::status(&a.f, manifest.id).await,
			Err(Error::NotFound(_))
		));
		for node in [&a, &b] {
			use reinhardt::query::{Alias, ColumnRef, Expr, Func, PostgresQueryBuilder, Query};
			let count: i64 = sqlx::query_scalar(
				&Query::select()
					.expr(Func::count(Expr::col(ColumnRef::Asterisk).into()))
					.from(Alias::new("atomic_participants"))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(node.f.store.control_pool.driver())
			.await
			.unwrap();
			assert_eq!(
				count, 0,
				"no full manifest is admitted before all disclosure preflights pass"
			);
		}
		if let Some((c, _)) = third {
			c.cleanup().await;
		}
		a.cleanup().await;
		b.cleanup().await;
		return;
	}
	assert_eq!(status, 202, "{body}");
	if revocation == "lost_reply" {
		use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
		let query = Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("authorization_decisions"))
			.to_string(PostgresQueryBuilder);
		let before_a: i64 = sqlx::query_scalar(&query)
			.fetch_one(a.f.store.pool.driver())
			.await
			.unwrap();
		let before_b: i64 = sqlx::query_scalar(&query)
			.fetch_one(b.f.store.pool.driver())
			.await
			.unwrap();
		for _ in 0..2 {
			let (status, rows) =
				common::request(&app_a, &token_a, "GET", "/api/transactions", Value::Null).await;
			assert_eq!(status, 200, "{rows}");
			assert_eq!(rows[0]["id"], json!(manifest.id));
		}
		assert_eq!(
			sqlx::query_scalar::<_, i64>(&query)
				.fetch_one(a.f.store.pool.driver())
				.await
				.unwrap(),
			before_a
		);
		assert_eq!(
			sqlx::query_scalar::<_, i64>(&query)
				.fetch_one(b.f.store.pool.driver())
				.await
				.unwrap(),
			before_b
		);
	}
	steps(
		&a,
		manifest.id,
		if transitions == usize::MAX {
			count
		} else {
			transitions
		},
	)
	.await;
	let mut reconciliation_check = None;
	match revocation {
		"lost_reply" => {
			// Act: delay the durable reservation reply through native middleware.
			b.stop().await;
			let admitted = fault_signals.admitted;
			let release = fault_signals.release;
			let reached = admitted.clone();
			let unblock = release.clone();
			let app = aidash_server::routes()
				.into_server()
				.with_di_context(b.application.context.clone())
				.with_middleware(ClosureMiddleware(
					move |request: reinhardt::Request, next: Arc<dyn reinhardt::Handler>| {
						let reached = reached.clone();
						let unblock = unblock.clone();
						async move {
							let reservation =
								request.uri.path() == "/federation/v0.1/transactions/reserve";
							let response = next.handle(request).await.unwrap();
							if reservation && response.status.is_success() {
								reached.notify_one();
								unblock.notified().await;
								return reinhardt::Response::new(
									http::StatusCode::SERVICE_UNAVAILABLE,
								);
							}
							response
						}
					},
				));
			b.serve(Arc::new(app)).await;
			// Act: use a fresh declared connection pool after replacing the participant transport.
			a.f.client = http_client;
			let f = a.f.clone();
			let id = manifest.id;
			let pending = tokio::spawn(async move { coordinator::advance(&f, id).await.unwrap() });
			tokio::time::timeout(std::time::Duration::from_secs(10), admitted.notified())
				.await
				.unwrap();
			// Retry the actually issued ticket with only one available control slot.
			// The source must read status through the retained authority transaction,
			// even while the original reservation reply is still unknown.
			let mut limited = a.f.clone();
			limited.store.control_pool =
				a.f.store
					.control_pool
					.options()
					.clone()
					.max_connections(1)
					.acquire_timeout(std::time::Duration::from_secs(2))
					.connect_with(a.f.store.control_pool.connect_options().as_ref().clone())
					.await
					.unwrap()
					.into();
			let ticket_app = common::application(limited.clone()).await;
			let peer_token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
			// Act: rebuild after constraining the durable control pool to one slot.
			for _ in 0..2 {
				let response = ticket_app
					.client()
					.get_with_headers(
						&format!("/federation/v0.1/transactions/{}/authority", manifest.id),
						&[
							("authorization", &format!("Bearer {peer_token}")),
							("x-aidash-node", &b.f.config.node_id),
							("x-aidash-protocol", "0.2"),
						],
					)
					.await
					.unwrap();
				let status = response.status_code();
				let bytes = response.body();
				assert_eq!(
					status,
					200,
					"ticket retry required another control slot: {}",
					String::from_utf8_lossy(bytes)
				);
				let ticket: Value = serde_json::from_slice(bytes).unwrap();
				assert_eq!(ticket["id"], json!(manifest.id));
				assert_eq!(ticket["digest"], manifest.digest().unwrap());
			}

			drop(ticket_app);
			limited.store.control_pool.close().await;
			let path = format!(
				"/api/authorization/acme/credentials/{}/revoke",
				credential_a.id
			);
			let (status, body) =
				common::request(&app_a, &a.f.config.api_token, "POST", &path, Value::Null).await;
			assert_eq!(
				status, 202,
				"an unknown durable admission cannot acknowledge complete revocation: {body}"
			);
			assert_eq!(body["pending_transactions"], json!([manifest.id]));
			let (status, trust) = a
				.request(
					reqwest::Method::POST,
					"/api/transactions/trust",
					Some(json!({"node_id":b.f.config.node_id,"enabled":false})),
				)
				.await;
			assert_eq!(status, 202, "{trust}");
			assert_eq!(trust["pending_transactions"], json!([manifest.id]));
			let (_, trusts) = a.get("/api/transactions/trust").await;
			let pending_trust = trusts
				.as_array()
				.unwrap()
				.iter()
				.find(|r| r["node_id"] == b.f.config.node_id)
				.unwrap();
			assert_eq!(pending_trust["pending_transactions"], json!([manifest.id]));

			assert_eq!(
				common::request(&app_a, &token_a, "GET", "/api/transactions", Value::Null)
					.await
					.0,
				401
			);
			release.notify_one();
			let unknown = pending.await.unwrap();
			assert!(
				unknown.decision.is_none(),
				"an unmarked 503 is not proof of a rejected admission"
			);
			reconciliation_check = Some(path);
		}
		"source" => {
			auth_a
				.revoke_credential("acme", credential_a.id)
				.await
				.unwrap();
		}
		"receiver" => {
			auth_b
				.revoke_credential("acme", credential_b.id)
				.await
				.unwrap();
		}
		"mapping" => {
			write(
				&b.f,
				"acme",
				PeerMappingInput {
					source_node: a.f.config.node_id.clone(),
					source_tenant: "acme".into(),
					source_subject: "alice".into(),
					credential_id: credential_b.id,
					enabled: false,
					expected_revision: 1,
				},
			)
			.await
			.unwrap();
		}
		"trust" => {
			let (status, body) = b
				.request(
					reqwest::Method::POST,
					"/api/transactions/trust",
					Some(json!({"node_id":a.f.config.node_id,"enabled":false})),
				)
				.await;
			assert_eq!(status, 200, "{body}");
		}
		_ => unreachable!(),
	}
	let result = complete(&a, manifest.id).await;
	assert_eq!(result.decision.as_deref(), Some(expected), "{result:?}");
	if let Some(path) = reconciliation_check {
		let (status, body) =
			common::request(&app_a, &a.f.config.api_token, "POST", &path, Value::Null).await;
		assert_eq!(status, 200, "{body}");
		assert_eq!(body["pending_transactions"], json!([]));
		let (_, trusts) = a.get("/api/transactions/trust").await;
		let trust = trusts
			.as_array()
			.unwrap()
			.iter()
			.find(|r| r["node_id"] == b.f.config.node_id)
			.unwrap();
		assert_eq!(trust["pending_transactions"], json!([]));
	}
	for (node, workspace) in [(&a, wa), (&b, wb)] {
		assert_eq!(
			node.f.store.workspace(workspace).await.unwrap().revision,
			i64::from(expected == "COMMIT")
		);
		assert_eq!(
			node.get(&format!("/api/workspaces/{workspace}")).await.0,
			200
		);
	}
	if let Some((c, workspace)) = third {
		assert_eq!(
			c.f.store.workspace(workspace).await.unwrap().revision,
			i64::from(expected == "COMMIT")
		);
		assert_eq!(c.get(&format!("/api/workspaces/{workspace}")).await.0, 200);
		c.cleanup().await;
	}
	a.cleanup().await;
	b.cleanup().await;
}

#[path = "transaction_protocol/process.rs"]
mod process;

#[path = "transaction_protocol/tiers.rs"]
mod tiers;

#[rstest::rstest]
#[tokio::test]
async fn restoring_the_same_peer_during_a_barrier_does_not_restore_transaction_trust(
	#[future(awt)] pair: Pair,
) {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let (a, b, manifest, wa, wb) = pair;
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 5).await;
	assert_eq!(
		coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("COMMIT")
	);
	// Simulate loss of the existing communication credential after admission.
	// The operator recovery route must restore that exact Node, not grant trust.
	let mut tx = a.f.store.control_pool.driver().begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.expr(Expr::cust(
				"set_config('aidash.transaction_control','authority',true)",
			))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	{
		let query_bind_1 = &b.f.config.node_id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("peers"))
				.value(Alias::new("enabled"), false)
				.and_where(SimpleExpr::CustomWithExpr(
					"(node_id=?)".into(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
	}
	.unwrap();
	tx.commit().await.unwrap();
	assert_eq!(
		a.request(
			reqwest::Method::POST,
			"/api/transactions/trust",
			Some(json!({"node_id":b.f.config.node_id,"enabled":false}))
		)
		.await
		.0,
		200
	);
	assert_eq!(
		b.request(
			reqwest::Method::POST,
			"/api/transactions/trust",
			Some(json!({"node_id":a.f.config.node_id,"enabled":false}))
		)
		.await
		.0,
		200
	);
	steps(&a, manifest.id, 2).await;
	assert!(
		!coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.complete
	);
	assert_eq!(a.get(&format!("/api/workspaces/{wa}")).await.0, 503);
	let (status, peer) = a
		.request(
			reqwest::Method::POST,
			"/api/transactions/peer-recovery",
			Some(json!({"node_id":b.f.config.node_id,"credential_env":"AIDASH_SECRET_TEST_PEER"})),
		)
		.await;
	assert_eq!(status, 200, "{peer}");
	assert_eq!(peer["endpoint"], b.f.config.endpoint);
	assert_eq!(
		a.get("/api/transactions/trust").await.1[0]["enabled"],
		false
	);
	assert_eq!(
		complete(&a, manifest.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	for (node, workspace) in [(&a, wa), (&b, wb)] {
		assert_eq!(node.f.store.workspace(workspace).await.unwrap().revision, 1);
	}
	let mut next = manifest.clone();
	next.id = Uuid::new_v4();
	for p in &mut next.participants {
		for mutation in &mut p.mutations {
			if let aidash_server::transactions::Mutation::WorkspaceState {
				expected_revision, ..
			} = mutation
			{
				*expected_revision = 1;
			}
		}
	}
	assert!(matches!(
		coordinator::submit(&a.f, &next).await,
		Err(Error::Forbidden)
	));
	a.cleanup().await;
	b.cleanup().await;
}

#[path = "transaction_protocol/listing.rs"]
mod listing;

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

#[fixture]
fn mapped_nodes(
	#[default(2)] count: usize,
	#[from(test_environment)] _environment: EnvironmentFuture,
	#[from(pair)]
	#[with(_environment.clone())]
	pair: PairFuture,
	#[from(node)]
	#[with("c",_environment.clone())]
	third: BoxFuture<'static, Node>,
) -> BoxFuture<'static, (Pair, Option<Node>)> {
	async move {
		(
			pair.await,
			if count == 3 { Some(third.await) } else { None },
		)
	}
	.boxed()
}
#[derive(Clone)]
struct FaultSignals {
	admitted: Arc<tokio::sync::Notify>,
	release: Arc<tokio::sync::Notify>,
	gate: Arc<tokio::sync::Semaphore>,
	barrier: Arc<tokio::sync::Barrier>,
	active: Arc<std::sync::atomic::AtomicUsize>,
	peak: Arc<std::sync::atomic::AtomicUsize>,
}
#[fixture]
fn fault_signals() -> FaultSignals {
	FaultSignals {
		gate: Arc::new(tokio::sync::Semaphore::new(0)),
		admitted: Arc::new(tokio::sync::Notify::new()),
		release: Arc::new(tokio::sync::Notify::new()),
		barrier: Arc::new(tokio::sync::Barrier::new(2)),
		active: Default::default(),
		peak: Default::default(),
	}
}
/// Type adapter for native middleware closures; all state is fixture-owned.
struct ClosureMiddleware<F>(F);
#[async_trait::async_trait]
impl<F, Fut> reinhardt::Middleware for ClosureMiddleware<F>
where
	F: Fn(reinhardt::Request, Arc<dyn reinhardt::Handler>) -> Fut + Send + Sync + 'static,
	Fut: std::future::Future<Output = reinhardt::Response> + Send + 'static,
{
	async fn process(
		&self,
		request: reinhardt::Request,
		next: Arc<dyn reinhardt::Handler>,
	) -> reinhardt::Result<reinhardt::Response> {
		Ok((self.0)(request, next).await)
	}
}

#[fixture]
fn event_streams() -> aidash_server::sse::Service {
	// No Outbox/NATS subscriber: bound reconciliation while observing the real visibility gate.
	aidash_server::sse::Service::new(aidash_server::sse::Settings {
		reconcile_interval: std::time::Duration::from_millis(250),
		..Default::default()
	})
}
