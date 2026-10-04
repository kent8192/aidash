use super::*;
use aidash_server::authorization::{
	Authorization,
	peer::{PeerMappingInput, write},
};
use axum::{Router, extract::Request, middleware::Next};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use std::time::Duration as StdDuration;
use tokio::sync::Semaphore;

struct ScopedPair {
	a: Node,
	b: Node,
	app_a: common::TestApplication,
	app_b: common::TestApplication,
	token_a: String,
	token_b: String,
	manifest: Manifest,
}

async fn map_subject(source: &Node, target: &Node) {
	let credential = Authorization {
		pool: target.f.store.control_pool.clone(),
	}
	.credentials("acme")
	.await
	.unwrap()
	.into_iter()
	.find(|credential| credential.subject == "alice")
	.unwrap();
	write(
		&target.f,
		"acme",
		PeerMappingInput {
			source_node: source.f.config.node_id.clone(),
			source_tenant: "acme".into(),
			source_subject: "alice".into(),
			credential_id: credential.id,
			enabled: true,
			expected_revision: 0,
		},
	)
	.await
	.unwrap();
}

async fn scoped_pair(environment: &TestEnvironment) -> ScopedPair {
	let (a, b, mut manifest, _, _) = pair(environment).await;
	let app_a = common::application(a.f.clone()).await;
	let app_b = common::application(b.f.clone()).await;
	let (_, token_a, task_a) = common::bootstrap(&a.f, &app_a, "http://localhost:1").await;
	let (_, token_b, task_b) = common::bootstrap(&b.f, &app_b, "http://localhost:1").await;
	map_subject(&a, &b).await;
	map_subject(&b, &a).await;
	for (index, workspace) in [
		a.f.store.task(task_a).await.unwrap().workspace_id,
		b.f.store.task(task_b).await.unwrap().workspace_id,
	]
	.into_iter()
	.enumerate()
	{
		manifest.participants[index].mutations =
			vec![aidash_server::transactions::Mutation::WorkspaceState {
				workspace_id: workspace,
				expected_revision: 0,
				state: json!({}),
			}];
	}
	ScopedPair {
		a,
		b,
		app_a,
		app_b,
		token_a,
		token_b,
		manifest,
	}
}

async fn serve(node: &mut Node, app: Router) {
	node.stop().await;
	let listener = tokio::net::TcpListener::bind(node.listen).await.unwrap();
	node.server = Some(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
}

#[rstest::rstest]
#[tokio::test]
async fn invalid_deadline_does_not_bind_remote_preflight(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[values(-60, 7200)] seconds: i64,
) {
	let ScopedPair {
		a,
		b,
		app_a,
		token_a,
		mut manifest,
		..
	} = scoped_pair(&environment).await;
	manifest.deadline = Utc::now() + Duration::seconds(seconds);
	let (status, body) = common::request(
		&app_a,
		&token_a,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(status, 400, "{body}");
	let bound: i64 = {
		let query_bind_1 = manifest.id;
		sqlx::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(
					Expr::col(Alias::new("id")).into(),
				))
				.from(Alias::new("atomic_preflights"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&b.f.store.control_pool)
		.await
	}
	.unwrap();
	manifest.deadline = Utc::now() + Duration::minutes(5);
	let (retry, body) = common::request(
		&app_a,
		&token_a,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	a.cleanup().await;
	b.cleanup().await;
	assert_eq!(
		bound, 0,
		"invalid admission must not strand a remote binding"
	);
	assert_eq!(
		retry, 202,
		"the same ID with a corrected deadline must work: {body}"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn deadline_expiring_during_preflight_has_a_recoverable_coordinator(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let ScopedPair {
		a,
		mut b,
		app_a,
		token_a,
		mut manifest,
		..
	} = scoped_pair(&environment).await;
	manifest.deadline = Utc::now() + Duration::seconds(3);
	let deadline = manifest.deadline;
	let app = common::application(b.f.clone())
		.await
		.test_transport()
		.layer(axum::middleware::from_fn(
			move |request: Request, next: Next| async move {
				if request.uri().path() == "/federation/v0.1/transactions/preflight" {
					let remaining = (deadline - Utc::now()).to_std().unwrap_or_default();
					tokio::time::sleep(remaining + StdDuration::from_millis(50)).await;
				}
				next.run(request).await
			},
		));
	serve(&mut b, app).await;
	let (status, body) = common::request(
		&app_a,
		&token_a,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(
		status, 202,
		"expiry during preflight must remain recoverable: {body}"
	);
	assert!(Utc::now() > manifest.deadline);
	let state = complete(&a, manifest.id).await;
	assert_eq!(state.decision.as_deref(), Some("ABORT"));
	let (retry, body) = common::request(
		&app_a,
		&token_a,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	a.cleanup().await;
	b.cleanup().await;
	assert_eq!(
		retry, 202,
		"expired idempotent retries must still work: {body}"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn unavailable_peer_hides_only_its_rows(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let ScopedPair {
		a,
		mut b,
		app_a,
		token_a,
		mut manifest,
		..
	} = scoped_pair(&environment).await;
	let (status, body) = common::request(
		&app_a,
		&token_a,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(status, 202, "{body}");
	let remote_id = manifest.id;
	manifest.id = Uuid::new_v4();
	manifest
		.participants
		.retain(|node| node.node_id == a.f.config.node_id);
	let (status, body) = common::request(
		&app_a,
		&token_a,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(status, 202, "{body}");
	b.stop().await;
	let (status, rows) =
		common::request(&app_a, &token_a, "GET", "/api/transactions", Value::Null).await;
	assert_eq!(status, 200, "{rows}");
	assert_eq!(rows.as_array().unwrap().len(), 1, "{rows}");
	assert_eq!(rows[0]["id"], json!(manifest.id));
	b.restart().await;
	let (status, rows) =
		common::request(&app_a, &token_a, "GET", "/api/transactions", Value::Null).await;
	a.cleanup().await;
	b.cleanup().await;
	assert_eq!(status, 200, "{rows}");
	let ids: Vec<_> = rows
		.as_array()
		.unwrap()
		.iter()
		.map(|row| row["id"].clone())
		.collect();
	assert_eq!(ids, vec![json!(manifest.id), json!(remote_id)]);
}

#[rstest::rstest]
#[tokio::test]
async fn bidirectional_submissions_reserve_inbound_control_capacity(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let ScopedPair {
		mut a,
		mut b,
		app_a,
		app_b,
		token_a,
		token_b,
		manifest,
	} = scoped_pair(&environment).await;
	let gate = Arc::new(Semaphore::new(0));
	let arrived = Arc::new(AtomicUsize::new(0));
	for node in [&mut a, &mut b] {
		let gate = gate.clone();
		let arrived = arrived.clone();
		let app = common::application(node.f.clone())
			.await
			.test_transport()
			.layer(axum::middleware::from_fn(
				move |request: Request, next: Next| {
					let gate = gate.clone();
					let arrived = arrived.clone();
					async move {
						if request.uri().path() == "/federation/v0.1/transactions/preflight" {
							arrived.fetch_add(1, Ordering::SeqCst);
							gate.acquire().await.unwrap().forget();
						}
						next.run(request).await
					}
				},
			));
		serve(node, app).await;
	}
	let mut requests = Vec::new();
	for _ in 0..16 {
		for (app, token, coordinator) in [
			(&app_a, &token_a, &a.f.config.node_id),
			(&app_b, &token_b, &b.f.config.node_id),
		] {
			let app = app.clone();
			let token = token.clone();
			let mut manifest = manifest.clone();
			manifest.id = Uuid::new_v4();
			manifest.coordinator = coordinator.clone();
			requests.push(tokio::spawn(async move {
				common::request(&app, &token, "POST", "/api/transactions", json!(manifest)).await
			}));
		}
	}
	tokio::time::timeout(StdDuration::from_secs(5), async {
		while arrived.load(Ordering::SeqCst) < 16 {
			tokio::time::sleep(StdDuration::from_millis(10)).await;
		}
	})
	.await
	.expect("both nodes must issue preflights concurrently");
	tokio::time::sleep(StdDuration::from_millis(100)).await;
	// A blocked outbound RPC may retain one connection, but not all sixteen.
	let spare_a = a.f.store.control_pool.size() as usize - a.f.store.control_pool.num_idle() < 16;
	let spare_b = b.f.store.control_pool.size() as usize - b.f.store.control_pool.num_idle() < 16;
	gate.add_permits(32);
	let results = tokio::time::timeout(
		StdDuration::from_secs(15),
		futures_util::future::join_all(requests),
	)
	.await
	.expect("bidirectional admissions must finish without pool starvation");
	a.cleanup().await;
	b.cleanup().await;
	assert!(
		spare_a && spare_b,
		"outbound admissions exhausted an inbound control pool"
	);
	for result in results {
		let (status, body) = result.unwrap();
		assert_eq!(status, 202, "{body}");
	}
}

#[rstest::rstest]
#[tokio::test]
async fn independent_preflights_run_concurrently(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let ScopedPair {
		a,
		mut b,
		app_a,
		token_a,
		mut manifest,
		..
	} = scoped_pair(&environment).await;
	let mut c = Node::new(&environment, "preflight-c").await;
	for (local, remote) in [(&a, &c), (&c, &a)] {
		local
			.f
			.register_peer(Peer {
				node_id: remote.f.config.node_id.clone(),
				endpoint: remote.f.config.endpoint.clone(),
				credential_env: "AIDASH_SECRET_TRANSACTION_02".into(),
				protocol_version: "0.1".into(),
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
	let (_, _, task) = common::bootstrap(
		&c.f,
		&common::application(c.f.clone()).await,
		"http://localhost:1",
	)
	.await;
	map_subject(&a, &c).await;
	let workspace = c.f.store.task(task).await.unwrap().workspace_id;
	let mut third = manifest.participants[0].clone();
	third.node_id = c.f.config.node_id.clone();
	third.mutations = vec![aidash_server::transactions::Mutation::WorkspaceState {
		workspace_id: workspace,
		expected_revision: 0,
		state: json!({}),
	}];
	manifest.participants.push(third);
	let barrier = Arc::new(tokio::sync::Barrier::new(2));
	for node in [&mut b, &mut c] {
		let barrier = barrier.clone();
		let app = common::application(node.f.clone())
			.await
			.test_transport()
			.layer(axum::middleware::from_fn(
				move |request: Request, next: Next| {
					let barrier = barrier.clone();
					async move {
						if request.uri().path() == "/federation/v0.1/transactions/preflight" {
							barrier.wait().await;
						}
						next.run(request).await
					}
				},
			));
		serve(node, app).await;
	}
	let response = tokio::time::timeout(
		StdDuration::from_secs(5),
		common::request(
			&app_a,
			&token_a,
			"POST",
			"/api/transactions",
			json!(manifest),
		),
	)
	.await;
	a.cleanup().await;
	b.cleanup().await;
	c.cleanup().await;
	let (status, body) = response.expect("independent participants must both reach preflight");
	assert_eq!(status, 202, "{body}");
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;
