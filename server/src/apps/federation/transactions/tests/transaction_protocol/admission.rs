use super::*;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::authorization::{Authorization, peer::write};
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
		target.mapping(source, credential.id, true, 0),
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
	for (index, thread) in [
		a.f.store
			.task(&TEST_FIXTURE, task_a)
			.await
			.unwrap()
			.thread_id,
		b.f.store
			.task(&TEST_FIXTURE, task_b)
			.await
			.unwrap()
			.thread_id,
	]
	.into_iter()
	.enumerate()
	{
		manifest.participants[index].mutations =
			vec![aidash_server::transactions::Mutation::ThreadTitle {
				thread_id: thread,
				expected_revision: 0,
				title: "Title".into(),
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
async fn an_invalid_deadline_is_refused_before_any_cross_deployment_preflight(
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
	let (status, body) = common::post(&app_a, &token_a, "/api/transactions", json!(manifest)).await;
	assert_eq!(status, 400, "{body}");
	assert_no_transaction_state(&[&a, &b], manifest.id).await;
	// A Deployment Administrator owns no transactions: it is refused before the
	// deadline is looked at (#484).
	let (status, body) = common::post(
		&app_a,
		&a.f.config.api_token,
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(status, 403, "{body}");
	assert_no_transaction_state(&[&a, &b], manifest.id).await;
	// The refused ID stays free: a corrected deadline submits it.
	manifest.deadline = Utc::now() + Duration::minutes(5);
	let (status, body) = common::post(
		&app_a,
		&a.f.config.api_token,
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(status, 403, "{body}");
	let (retry, body) = common::post(&app_a, &token_a, "/api/transactions", json!(manifest)).await;
	assert_eq!(retry, 202, "{body}");
	let state = complete(&a, manifest.id).await;
	assert_eq!(state.decision.as_deref(), Some("COMMIT"));
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_preflight_that_outlasts_the_deadline_aborts_and_stays_retryable(
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
	let (status, body) = common::post(&app_a, &token_a, "/api/transactions", json!(manifest)).await;
	assert_eq!(status, 202, "{body}");
	assert_eq!(preflights.load(Ordering::SeqCst), 1);
	// The admitted transaction is recoverable after its deadline, and aborts.
	let remaining = (deadline - Utc::now()).to_std().unwrap_or_default();
	tokio::time::sleep(remaining + StdDuration::from_millis(50)).await;
	assert!(Utc::now() > manifest.deadline);
	let state = complete(&a, manifest.id).await;
	assert_eq!(state.decision.as_deref(), Some("ABORT"));
	let (retry, body) = common::post(&app_a, &token_a, "/api/transactions", json!(manifest)).await;
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
	let (status, body) = common::post(&app_a, &token_a, "/api/transactions", json!(manifest)).await;
	assert_eq!(status, 202, "{body}");
	let remote_id = manifest.id;
	manifest.id = Uuid::new_v4();
	manifest
		.participants
		.retain(|node| node.node_id == a.address);
	let (status, body) = common::post(&app_a, &token_a, "/api/transactions", json!(manifest)).await;
	assert_eq!(status, 202, "{body}");
	b.stop().await;
	let (status, rows) = common::get(&app_a, &token_a, "/api/transactions").await;
	assert_eq!(status, 200, "{rows}");
	assert_eq!(rows.as_array().unwrap().len(), 1, "{rows}");
	assert_eq!(rows[0]["id"], json!(manifest.id));
	b.restart().await;
	let (status, rows) = common::get(&app_a, &token_a, "/api/transactions").await;
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

/// Each Node submits sixteen transactions naming the other at once. Every
/// submission preflights on the other Node while holding its own authority
/// locks, so nested Federation calls must never wait on each other: each one
/// settles within the bound, admitted or refused as retryable, and a refused
/// one leaves nothing behind.
#[rstest::rstest]
#[tokio::test]
async fn bidirectional_new_scoped_submissions_settle_without_a_lock_cycle(
	#[future(awt)] scoped_pair: ScopedPair,
) {
	let ScopedPair {
		a,
		b,
		app_a,
		app_b,
		token_a,
		token_b,
		manifest,
	} = scoped_pair;
	let mut requests = Vec::new();
	for _ in 0..16 {
		for (app, token, coordinator) in [
			(&app_a, &token_a, a.address.as_str()),
			(&app_b, &token_b, b.address.as_str()),
		] {
			let app = app.clone();
			let token = token.clone();
			let mut manifest = manifest.clone();
			manifest.id = Uuid::new_v4();
			manifest.coordinator = coordinator.to_owned();
			requests.push(tokio::spawn(async move {
				let id = manifest.id;
				(
					id,
					common::post(&app, &token, "/api/transactions", json!(manifest)).await,
				)
			}));
		}
	}
	let results = tokio::time::timeout(
		StdDuration::from_secs(60),
		futures_util::future::join_all(requests),
	)
	.await
	.expect("concurrent submissions in both directions must not wait on each other");
	for result in results {
		let (id, (status, body)) = result.unwrap();
		assert!(matches!(status, 202 | 503), "{status}: {body}");
		if status == 503 {
			assert_no_transaction_state(&[&a, &b], id).await;
		}
	}
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn multi_peer_new_scoped_submissions_preflight_every_peer_concurrently(
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
	c.join().await;
	common::register_pair(
		(&a.f, &a.application, "acme"),
		(&c.f, &c.application, "acme"),
	)
	.await;
	let (_, _, task) = common::bootstrap(&c.f, &c.application, "http://localhost:1").await;
	map_subject(&a, &c).await;
	let thread = c.f.store.task(&TEST_FIXTURE, task).await.unwrap().thread_id;
	let mut third = manifest.participants[0].clone();
	third.node_id = c.address.clone();
	third.mutations = vec![aidash_server::transactions::Mutation::ThreadTitle {
		thread_id: thread,
		expected_revision: 0,
		title: "Title".into(),
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
		common::post(&app_a, &token_a, "/api/transactions", json!(manifest)),
	)
	.await;
	// The barrier opens only when both peers' preflights are in flight.
	let (status, body) = response.expect("peer preflights must run concurrently");
	assert_eq!(status, 202, "{body}");
	assert_eq!(preflights.load(Ordering::SeqCst), 2);
	let state = complete(&a, manifest.id).await;
	assert_eq!(state.decision.as_deref(), Some("COMMIT"));
	assert_only_own_parts(&[&a, &b, &c], &manifest).await;
	a.cleanup().await;
	b.cleanup().await;
	c.cleanup().await;
}
