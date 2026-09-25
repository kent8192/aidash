use super::*;

#[rstest::fixture]
async fn network_fixture(
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) -> (CoreFixture, aidash::domain::Run) {
	let c = Box::pin(build_core_fixture(
		test_environment,
		"aidash://network-test",
	))
	.await;
	let (mut c, run) = Box::pin(configure_approvals(c)).await;
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.outbound_origins = vec![
		"https://httpbingo.org".into(),
		"https://example.com".into(),
		"https://127.0.0.1.nip.io".into(),
	];
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	(c, run)
}
async fn approve(c: &CoreFixture, run: Uuid, url: &str, reusable: bool) -> (Value, Value) {
	let input = json!({"idempotency_key":Uuid::new_v4(),"url":url});
	let (status, pending) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{run}/outbound"),
		input.clone(),
	)
	.await;
	assert_eq!(status, 200, "{pending}");
	assert_eq!(pending["status"], "approval_required");
	let mut decision = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1});
	if reusable {
		decision["choice"] = json!("allow_run");
		decision["targets"] = pending["targets"].clone();
		decision["expires_at"] = json!(chrono::Utc::now() + chrono::Duration::minutes(5));
	}
	let result = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/capabilities/approvals/{}/decide",
			pending["approval_id"].as_str().unwrap()
		),
		decision,
	)
	.await;
	assert_eq!(result.0, 200, "{}", result.1);
	(input, result.1)
}
async fn outcome(c: &CoreFixture, run: Uuid, input: &Value, state: &str) -> Value {
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
	loop {
		let (status, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/runs/{run}/outbound"),
			input.clone(),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		if result["status"] == state {
			return result;
		}
		assert_eq!(result["status"], "running", "{result}");
		assert!(tokio::time::Instant::now() < deadline, "{result}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
}
#[rstest::rstest]
#[tokio::test]
async fn real_https_redirects_stay_inside_the_grant_and_private_dns_is_denied(
	#[future] network_fixture: (CoreFixture, aidash::domain::Run),
) {
	let (c, run) = Box::pin(network_fixture).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let (input, _) = approve(
		&c,
		run.id,
		"https://httpbingo.org/relative-redirect/1",
		false,
	)
	.await;
	let done = outcome(&c, run.id, &input, "completed").await;
	assert_eq!(done["http_status"], 200);
	assert!(
		done["output"]
			.as_str()
			.unwrap()
			.contains("https://httpbingo.org/get")
	);
	// The operator permits both hosts, but this invocation approved one origin.
	let (redirect, _) = approve(
		&c,
		run.id,
		"https://httpbingo.org/redirect-to?url=https%3A%2F%2Fexample.com",
		false,
	)
	.await;
	let blocked = outcome(&c, run.id, &redirect, "uncertain").await;
	assert_eq!(blocked["error"]["code"], "OUTBOUND_STOPPED");
	assert!(blocked["output_file"].is_null());
	// A valid HTTPS origin with a private DNS answer must never reach TLS or HTTP.
	let (private, _) = approve(&c, run.id, "https://127.0.0.1.nip.io/", false).await;
	let blocked = outcome(&c, run.id, &private, "uncertain").await;
	assert!(blocked["output_file"].is_null());
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/runs/{}/outbound", run.id),
			json!({"idempotency_key":Uuid::new_v4(),"url":"https://sentinel-secret@httpbingo.org/get"})
		)
		.await
		.0,
		400
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
#[rstest::rstest]
#[tokio::test]
async fn revocation_withdraws_a_real_pending_https_channel_and_does_not_replay_it(
	#[future] network_fixture: (CoreFixture, aidash::domain::Run),
) {
	let (c, run) = Box::pin(network_fixture).await;
	let (input, allowed) = approve(&c, run.id, "https://httpbingo.org/delay/9", true).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
	loop {
		let (_, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/runs/{}/outbound", run.id),
			input.clone(),
		)
		.await;
		if result["effects_may_have_occurred"] == true {
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{result}");
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}
	// Give the real request time to reach the delayed endpoint before revocation.
	tokio::time::sleep(std::time::Duration::from_millis(500)).await;
	let began = tokio::time::Instant::now();
	let revoked = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/capabilities/grants/{}/revoke",
			allowed["grant_id"].as_str().unwrap()
		),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1}),
	)
	.await;
	assert_eq!(revoked.0, 200, "{}", revoked.1);
	let denied = outcome(&c, run.id, &input, "uncertain").await;
	assert!(
		began.elapsed() < std::time::Duration::from_secs(5),
		"revocation must interrupt the nine-second response"
	);
	assert_eq!(denied["effects_may_have_occurred"], true);
	assert!(denied["output_file"].is_null());
	let retry = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/outbound", run.id),
		input.clone(),
	)
	.await;
	assert_eq!(
		retry.1, denied,
		"uncertain requests never replay automatically"
	);
	let (_, next) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/outbound", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://httpbingo.org/get"}),
	)
	.await;
	assert_eq!(next["status"], "approval_required");
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
