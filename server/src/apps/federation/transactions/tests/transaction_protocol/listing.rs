use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[rstest::rstest]
#[tokio::test]
async fn listing_checks_peers_concurrently_and_preserves_order_and_denials(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use aidash_server::authorization::{
		Authorization,
		peer::{PeerMappingInput, write},
	};
	let (a, mut b, mut manifest, _, _) = pair(&environment).await;
	let app_a = common::application(a.f.clone()).await;
	let app_b = common::application(b.f.clone()).await;
	let (_, token, task_a) = common::bootstrap(&a.f, &app_a, "http://localhost:1").await;
	let (_, _, task_b) = common::bootstrap(&b.f, &app_b, "http://localhost:1").await;
	let auth = Authorization {
		pool: b.f.store.control_pool.clone(),
	};
	let credential = auth
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
			credential_id: credential.id,
			enabled: true,
			expected_revision: 0,
		},
	)
	.await
	.unwrap();
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
	let mut expected = Vec::new();
	for _ in 0..12 {
		manifest.id = Uuid::new_v4();
		let (status, body) =
			common::request(&app_a, &token, "POST", "/api/transactions", json!(manifest)).await;
		assert_eq!(status, 202, "{body}");
		expected.push(json!(manifest.id));
	}
	expected.reverse();
	b.stop().await;
	let active = Arc::new(AtomicUsize::new(0));
	let peak = Arc::new(AtomicUsize::new(0));
	let observed = peak.clone();
	// Delay actual peer authorization responses, without replacing its decision.
	let app = common::application(b.f.clone())
		.await
		.test_transport()
		.layer(axum::middleware::from_fn(
			move |request: axum::extract::Request, next: axum::middleware::Next| {
				let active = active.clone();
				let peak = observed.clone();
				async move {
					let check = request.uri().path() == "/federation/v0.1/transactions/access";
					if check {
						peak.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
						tokio::time::sleep(std::time::Duration::from_millis(100)).await;
					}
					let response = next.run(request).await;
					if check {
						active.fetch_sub(1, Ordering::SeqCst);
					}
					response
				}
			},
		));
	let listener = tokio::net::TcpListener::bind(b.listen).await.unwrap();
	b.server = Some(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let (status, rows) =
		common::request(&app_a, &token, "GET", "/api/transactions", Value::Null).await;
	assert_eq!(status, 200, "{rows}");
	let actual: Vec<_> = rows
		.as_array()
		.unwrap()
		.iter()
		.map(|row| row["id"].clone())
		.collect();
	assert_eq!(actual, expected);
	let concurrency = peak.load(Ordering::SeqCst);
	auth.revoke_credential("acme", credential.id).await.unwrap();
	let (status, rows) =
		common::request(&app_a, &token, "GET", "/api/transactions", Value::Null).await;
	assert_eq!(status, 200, "{rows}");
	assert_eq!(
		rows,
		json!([]),
		"revoked remote authority must hide every row"
	);
	a.cleanup().await;
	b.cleanup().await;
	assert!(
		(2..=8).contains(&concurrency),
		"peer checks must be bounded and concurrent, observed {concurrency}"
	);
}

#[path = "admission.rs"]
mod admission;
