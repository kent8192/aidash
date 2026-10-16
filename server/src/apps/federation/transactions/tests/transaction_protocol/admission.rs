use super::*;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::authorization::{
	Authorization,
	peer::{PeerMappingInput, write},
};
use std::time::Duration as StdDuration;

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

#[fixture]
async fn scoped_pair(#[future(awt)] pair: Pair) -> ScopedPair {
	let (a, b, mut manifest, _, _) = pair;
	let app_a = a.application.clone();
	let app_b = b.application.clone();
	let (_, token_a, task_a) = common::bootstrap(&a.f, &app_a, "http://localhost:1").await;
	let (_, token_b, task_b) = common::bootstrap(&b.f, &app_b, "http://localhost:1").await;
	map_subject(&a, &b).await;
	map_subject(&b, &a).await;
	for (index, workspace) in [
		a.f.store
			.task(&TEST_FIXTURE, task_a)
			.await
			.unwrap()
			.workspace_id,
		b.f.store
			.task(&TEST_FIXTURE, task_b)
			.await
			.unwrap()
			.workspace_id,
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

async fn serve(node: &mut Node, app: reinhardt::ServerRouter) {
	// Act: install the fault at the participant stable origin after domain admission setup.
	node.serve(Arc::new(app)).await;
}

#[rstest::rstest]
#[tokio::test]
async fn invalid_deadline_cannot_bypass_cross_node_submission_suspension(
	#[future(awt)] scoped_pair: ScopedPair,
	#[values(-60, 7200)] seconds: i64,
) {
	let ScopedPair {
		a,
		b,
		app_a,
		token_a,
		mut manifest,
		..
	} = scoped_pair;
	manifest.deadline = Utc::now() + Duration::seconds(seconds);
	let (status, body) = common::request(
		&app_a,
		&token_a,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(status, 403, "{body}");
	assert_no_transaction_state(&[&a, &b], manifest.id).await;
	// The Deployment Administrator route still performs normal deadline validation.
	let (status, body) = common::request(
		&app_a,
		&a.f.config.api_token,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(status, 400, "{body}");
	assert_no_transaction_state(&[&a, &b], manifest.id).await;
	manifest.deadline = Utc::now() + Duration::minutes(5);
	let (retry, body) = common::request(
		&app_a,
		&token_a,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(
		retry, 403,
		"correcting the deadline must not bypass suspension: {body}"
	);
	assert_no_transaction_state(&[&a, &b], manifest.id).await;
	let (retry, body) = common::request(
		&app_a,
		&a.f.config.api_token,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(
		retry, 202,
		"the Deployment Administrator can reuse the corrected ID: {body}"
	);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn suspended_submission_keeps_expired_existing_transactions_recoverable(
	#[future(awt)] scoped_pair: ScopedPair,
) {
	let ScopedPair {
		a,
		mut b,
		app_a,
		token_a,
		mut manifest,
		..
	} = scoped_pair;
	manifest.deadline = Utc::now() + Duration::seconds(3);
	let deadline = manifest.deadline;
	let preflights = Arc::new(AtomicUsize::new(0));
	let observed = preflights.clone();
	let app = aidash_server::routes()
		.into_server()
		.with_di_context(b.application.context.clone())
		.with_middleware(ClosureMiddleware(
			move |request: reinhardt::Request, next: Arc<dyn reinhardt::Handler>| {
				let observed = observed.clone();
				async move {
					if request.uri.path() == "/federation/v0.1/transactions/preflight" {
						observed.fetch_add(1, Ordering::SeqCst);
						let remaining = (deadline - Utc::now()).to_std().unwrap_or_default();
						tokio::time::sleep(remaining + StdDuration::from_millis(50)).await;
					}
					next.handle(request).await.unwrap()
				}
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
		status, 403,
		"fresh scoped submission must be suspended: {body}"
	);
	assert_eq!(preflights.load(Ordering::SeqCst), 0);
	assert_no_transaction_state(&[&a, &b], manifest.id).await;
	// Existing durable scoped work remains recoverable after its deadline.
	seed_existing_scoped_transaction(&a, &manifest, &[&a, &b]).await;
	let remaining = (deadline - Utc::now()).to_std().unwrap_or_default();
	tokio::time::sleep(remaining + StdDuration::from_millis(50)).await;
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
async fn unavailable_peer_hides_only_its_rows(#[future(awt)] scoped_pair: ScopedPair) {
	let ScopedPair {
		a,
		mut b,
		app_a,
		token_a,
		mut manifest,
		..
	} = scoped_pair;
	seed_existing_scoped_transaction(&a, &manifest, &[&a, &b]).await;
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
async fn bidirectional_new_scoped_submissions_are_rejected_before_preflight(
	#[future(awt)] scoped_pair: ScopedPair,

	fault_signals: FaultSignals,
) {
	let ScopedPair {
		mut a,
		mut b,
		app_a,
		app_b,
		token_a,
		token_b,
		manifest,
	} = scoped_pair;
	let gate = fault_signals.gate;
	let arrived = fault_signals.active;
	for node in [&mut a, &mut b] {
		let gate = gate.clone();
		let arrived = arrived.clone();
		let app = aidash_server::routes()
			.into_server()
			.with_di_context(node.application.context.clone())
			.with_middleware(ClosureMiddleware(
				move |request: reinhardt::Request, next: Arc<dyn reinhardt::Handler>| {
					let gate = gate.clone();
					let arrived = arrived.clone();
					async move {
						if request.uri.path() == "/federation/v0.1/transactions/preflight" {
							arrived.fetch_add(1, Ordering::SeqCst);
							gate.acquire().await.unwrap().forget();
						}
						next.handle(request).await.unwrap()
					}
				},
			));
		serve(node, app).await;
	}
	let mut ids = Vec::new();
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
			ids.push(manifest.id);
			manifest.coordinator = coordinator.clone();
			requests.push(tokio::spawn(async move {
				common::request(&app, &token, "POST", "/api/transactions", json!(manifest)).await
			}));
		}
	}
	let results = tokio::time::timeout(
		StdDuration::from_secs(5),
		futures_util::future::join_all(requests),
	)
	.await
	.expect("both nodes must reject suspended submissions without peer I/O");
	assert_eq!(
		arrived.load(Ordering::SeqCst),
		0,
		"no remote preflight may be sent"
	);
	for result in results {
		let (status, body) = result.unwrap();
		assert_eq!(status, 403, "{body}");
	}
	for id in ids {
		assert_no_transaction_state(&[&a, &b], id).await;
	}
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn multi_peer_new_scoped_submissions_are_rejected_before_preflight(
	#[future(awt)] scoped_pair: ScopedPair,

	#[future(awt)]
	#[from(node)]
	#[with("preflight-c")]
	third: Node,

	fault_signals: FaultSignals,
) {
	let ScopedPair {
		a,
		mut b,
		app_a,
		token_a,
		mut manifest,
		..
	} = scoped_pair;
	let mut c = third;
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
	let (_, _, task) = common::bootstrap(&c.f, &c.application, "http://localhost:1").await;
	map_subject(&a, &c).await;
	let workspace =
		c.f.store
			.task(&TEST_FIXTURE, task)
			.await
			.unwrap()
			.workspace_id;
	let mut third = manifest.participants[0].clone();
	third.node_id = c.f.config.node_id.clone();
	third.mutations = vec![aidash_server::transactions::Mutation::WorkspaceState {
		workspace_id: workspace,
		expected_revision: 0,
		state: json!({}),
	}];
	manifest.participants.push(third);
	let preflights = Arc::new(AtomicUsize::new(0));
	let barrier = fault_signals.barrier;
	for node in [&mut b, &mut c] {
		let barrier = barrier.clone();
		let preflights = preflights.clone();
		let app = aidash_server::routes()
			.into_server()
			.with_di_context(node.application.context.clone())
			.with_middleware(ClosureMiddleware(
				move |request: reinhardt::Request, next: Arc<dyn reinhardt::Handler>| {
					let barrier = barrier.clone();
					let preflights = preflights.clone();
					async move {
						if request.uri.path() == "/federation/v0.1/transactions/preflight" {
							preflights.fetch_add(1, Ordering::SeqCst);
							barrier.wait().await;
						}
						next.handle(request).await.unwrap()
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
	let (status, body) = response.expect("suspension must reject without any peer preflight");
	assert_eq!(status, 403, "{body}");
	assert_eq!(preflights.load(Ordering::SeqCst), 0);
	assert_no_transaction_state(&[&a, &b, &c], manifest.id).await;
	a.cleanup().await;
	b.cleanup().await;
	c.cleanup().await;
}
