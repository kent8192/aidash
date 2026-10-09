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
	config::Config,
	federation::{Federation, Peer},
	registry::Registry,
	transactions::{Manifest, coordinator, participant},
};
use chrono::{Duration, Utc};
use common::{TestEnvironment, test_environment};
use reinhardt::db::backends::{DatabaseConnection, dialect::PostgresBackend};
use reinhardt::db::orm::DatabaseConnectionLease;
use serde_json::{Value, json};
use sqlx::{Connection, Executor, postgres::PgConnection};
use std::sync::Arc;
use uuid::Uuid;
use worker_process::WorkerProcess;

struct Node {
	f: Federation,
	server: Option<tokio::task::JoinHandle<()>>,
	listen: std::net::SocketAddr,
	// Real-process cases keep this socket owned across stop, SIGKILL and exec.
	reservation: Option<std::net::TcpListener>,
	_capacity: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Node {
	async fn new(environment: &TestEnvironment, suffix: &str) -> Self {
		Self::with_port_reservation(environment, suffix, false).await
	}
	async fn with_port_reservation(
		environment: &TestEnvironment,
		suffix: &str,
		reserve_port: bool,
	) -> Self {
		let admin = environment.database_url.clone();
		let database = format!(
			"atomic_{}_{}",
			suffix.replace('-', "_"),
			Uuid::new_v4().simple()
		);
		PgConnection::connect(&admin)
			.await
			.unwrap()
			.execute(
				// SeaQuery has no CREATE/DROP DATABASE builder.
				format!("CREATE DATABASE {database}").as_str(),
			)
			.await
			.unwrap();
		let mut url = reqwest::Url::parse(&admin).unwrap();
		url.set_path(&format!("/{database}"));
		let node_id = format!("aidash://atomic-{suffix}");
		let store = common::native_store(url.as_str(), &node_id).await;
		let (listener, reservation) = if reserve_port {
			let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
			reservation.set_nonblocking(true).unwrap();
			let listener =
				tokio::net::TcpListener::from_std(reservation.try_clone().unwrap()).unwrap();
			(listener, Some(reservation))
		} else {
			(
				tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
				None,
			)
		};
		let listen = listener.local_addr().unwrap();
		let config = Config {
			node_id,
			endpoint: format!("http://{listen}"),
			database_url: url.to_string(),
			nats_url: environment.nats_url.clone(),
			api_token: "atomic-operator-fixture-token".into(),
			web_dir: "web/dist".into(),
			lease_seconds: 30,
			default_host_packages: vec![],
			oidc: None,
		};
		let f = Federation {
			sandbox: Default::default(),
			registry: Registry::new(store.pool.clone(), &store.node_id).unwrap(),
			store,
			config,
			client: reqwest::Client::new(),
			notify: Arc::new(tokio::sync::Notify::new()),
		};
		let app = common::application(f.clone()).await.test_transport();
		let server = Some(tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap()
		}));
		Self {
			f,
			listen,
			reservation,
			server,
			_capacity: None,
		}
	}
	async fn stop(&mut self) {
		if let Some(server) = self.server.take() {
			server.abort();
			let _ = server.await;
		}
		if self.reservation.is_some() {
			// Child readiness must use a fresh connection to the inherited socket,
			// rather than an idle keep-alive connection to the previous server.
			self.f.client = reqwest::Client::new();
		}
	}
	async fn restart(&mut self) {
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
		let listener = self.listener().await;
		let app = common::application(self.f.clone()).await.test_transport();
		self.server = Some(tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap()
		}));
	}
	async fn listener(&self) -> tokio::net::TcpListener {
		match &self.reservation {
			Some(listener) => {
				tokio::net::TcpListener::from_std(listener.try_clone().unwrap()).unwrap()
			}
			None => tokio::net::TcpListener::bind(self.listen)
				.await
				.unwrap_or_else(|error| {
					panic!(
						"fixture {} bind {}: {error}",
						self.f.config.node_id, self.listen
					)
				}),
		}
	}
	async fn request(
		&self,
		method: reqwest::Method,
		path: &str,
		body: Option<Value>,
	) -> (u16, Value) {
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
		let status = response.status().as_u16();
		let bytes = response.bytes().await.unwrap();
		(
			status,
			serde_json::from_slice(&bytes).unwrap_or(Value::Null),
		)
	}
	async fn get(&self, path: &str) -> (u16, Value) {
		self.request(reqwest::Method::GET, path, None).await
	}
	async fn cleanup(mut self) {
		self.stop().await;
		self.f.store.pool.close().await;
		self.f.store.control_pool.close().await;
		// TestEnvironment owns the disposable postmaster and all isolated
		// databases. Match the shared fixture cleanup: DROP DATABASE emits a
		// global process barrier and can block another test's PGroonga index
		// initialization. The final environment owner removes the container.
	}
}
impl Drop for Node {
	fn drop(&mut self) {
		if let Some(server) = &self.server {
			server.abort();
		}
	}
}
async fn pair(environment: &TestEnvironment) -> (Node, Node, Manifest, Uuid, Uuid) {
	pair_with_port_reservation(environment, false).await
}
async fn pair_with_port_reservation(
	environment: &TestEnvironment,
	reserve_ports: bool,
) -> (Node, Node, Manifest, Uuid, Uuid) {
	static CAPACITY: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
		std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));
	let capacity = CAPACITY.clone().acquire_owned().await.unwrap();
	let mut a = Node::with_port_reservation(environment, "a", reserve_ports).await;
	a._capacity = Some(capacity);
	let b = Node::with_port_reservation(environment, "b", reserve_ports).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (a, b, manifest, wa, wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (a, b, manifest, wa, wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (a, b, mut manifest, wa, wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (mut a, mut b, manifest, wa, wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (a, b, first, wa, wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use aidash_server::{
		domain::{NewTask, qualified_agent},
		registry::Entry,
	};
	let (a, b, mut manifest, wa, _wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (mut a, b, manifest, wa, wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use axum::{body::Body, http::Request};
	use futures_util::StreamExt;
	let (a, b, manifest, wa, _wb) = pair(&_test_environment).await;
	// This fixture has no Outbox publisher or NATS subscriber. Use a bounded
	// reconciliation cadence to exercise the visibility gate without waiting
	// for the production five-second missed-notification fallback.
	let event_streams = aidash_server::sse::Service::new(aidash_server::sse::Settings {
		reconcile_interval: std::time::Duration::from_millis(250),
		..Default::default()
	});
	let response = common::application_with_event_streams(
		a.f.clone(),
		Default::default(),
		event_streams.clone(),
	)
	.await
	.oneshot(
		Request::builder()
			.uri(format!("/api/events/stream?workspace_id={wa}"))
			.header("authorization", format!("Bearer {}", a.f.config.api_token))
			.body(Body::empty())
			.unwrap(),
	)
	.await
	.unwrap();
	assert_eq!(response.status(), 200);
	let mut stream = response.into_body().into_data_stream();
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (a, b, manifest, wa, wb) = pair(&_test_environment).await;
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
async fn every_durable_transition_survives_fresh_pools_and_http_servers(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (mut a, mut b, manifest, wa, wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (a, b, mut manifest, wa, wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (a, mut b, manifest, wa, wb) = pair(&_test_environment).await;
	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 5).await;
	b.stop().await;
	let mut worker = WorkerProcess::start(&a.f, &a.f.config.database_url, "public");
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
	let mut restarted_a = WorkerProcess::start(&a.f, &a.f.config.database_url, "public");
	let mut restarted_b = WorkerProcess::start(&b.f, &b.f.config.database_url, "public");
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
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (a, mut b, manifest, wa, _wb) = pair(&_test_environment).await;
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
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] transitions: usize,
	#[case] revocation: &str,
	#[case] expected: &str,
	#[values(2, 3)] count: usize,
) {
	use aidash_server::authorization::{
		Authorization,
		peer::{PeerMappingInput, write},
	};
	let (mut a, mut b, mut manifest, _, _) = pair(&environment).await;
	let app_a = common::application(a.f.clone()).await;
	let app_b = common::application(b.f.clone()).await;
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
	if count == 3 {
		let c = Node::new(&environment, "c").await;
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
		let app_c = common::application(c.f.clone()).await;
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
			use axum::{middleware, response::IntoResponse};
			b.stop().await;
			let admitted = Arc::new(tokio::sync::Notify::new());
			let release = Arc::new(tokio::sync::Notify::new());
			let reached = admitted.clone();
			let unblock = release.clone();
			let app = common::application(b.f.clone())
				.await
				.test_transport()
				.layer(middleware::from_fn(
					move |request: axum::extract::Request, next: middleware::Next| {
						let reached = reached.clone();
						let unblock = unblock.clone();
						async move {
							let reservation =
								request.uri().path() == "/federation/v0.1/transactions/reserve";
							let response = next.run(request).await;
							if reservation && response.status().is_success() {
								reached.notify_one();
								unblock.notified().await;
								return http::StatusCode::SERVICE_UNAVAILABLE.into_response();
							}
							response
						}
					},
				));
			let listener = b.listener().await;
			b.server = Some(tokio::spawn(async move {
				axum::serve(listener, app).await.unwrap()
			}));
			a.f.client = reqwest::Client::new();
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
			for _ in 0..2 {
				let response = ticket_app
					.clone()
					.oneshot(
						http::Request::builder()
							.uri(format!(
								"/federation/v0.1/transactions/{}/authority",
								manifest.id
							))
							.header("authorization", format!("Bearer {peer_token}"))
							.header("x-aidash-node", &b.f.config.node_id)
							.header("x-aidash-protocol", "0.2")
							.body(axum::body::Body::empty())
							.unwrap(),
					)
					.await
					.unwrap();
				let status = response.status();
				let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
					.await
					.unwrap();
				assert_eq!(
					status,
					200,
					"ticket retry required another control slot: {}",
					String::from_utf8_lossy(&bytes)
				);
				let ticket: Value = serde_json::from_slice(&bytes).unwrap();
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
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let (a, b, manifest, wa, wb) = pair(&environment).await;
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
