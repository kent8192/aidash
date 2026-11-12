//! Live previews: status, tickets, limits and the lifecycle stops that need no Pod.
use super::*;
use chrono::{Duration, Utc};

async fn working_area(c: &CoreFixture, run: Uuid) -> (Uuid, String) {
	let (status, created) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"index.html":null},"patch":"*** Begin Patch\n*** Add File: index.html\n+<html>live</html>\n*** End Patch"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{run}/working-area"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let id = serde_json::from_value(area["id"].clone()).unwrap();
	let digest: String = serde_json::from_value(
		area["manifest"]
			.as_array()
			.unwrap()
			.iter()
			.find(|file| file["path"] == "index.html")
			.unwrap()["digest"]
			.clone(),
	)
	.unwrap();
	(id, digest)
}

#[allow(clippy::too_many_arguments)]
async fn insert_running(
	c: &CoreFixture,
	id: Uuid,
	run: Uuid,
	area: Uuid,
	revision: i64,
	state: &str,
) {
	let now = Utc::now();
	sqlx::query(
		"INSERT INTO core_live_previews
		 (id, tenant, run_id, area_id, revision, port, state, idle_seconds,
		  absolute_seconds, started_at, last_request_at, expires_at, pod_host,
		  termination_confirmed)
		 VALUES ($1, 'acme', $2, $3, $4, 3000, $5, 900, 7200, $6, $6, $7, '10.0.0.7', false)",
	)
	.bind(id)
	.bind(run)
	.bind(area)
	.bind(revision)
	.bind(state)
	.bind(now)
	.bind(now + Duration::seconds(7200))
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
}

async fn live_row(c: &CoreFixture, id: Uuid) -> (String, Option<String>, bool) {
	sqlx::query_as(
		"SELECT state, stop_reason, termination_confirmed FROM core_live_previews WHERE id = $1",
	)
	.bind(id)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn a_run_without_a_live_preview_reads_null(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, body) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/live-preview", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body, json!({"live_preview": null, "may_stop": true}));
}

#[rstest::rstest]
#[tokio::test]
async fn missing_runs_and_administrator_refusals_are_distinct(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	for (token, expected) in [(&c.token, 404), (&c.f.config.api_token, 403)] {
		for (method, suffix) in [("GET", ""), ("POST", "/stop")] {
			let (status, body) = request(
				&c.app,
				token,
				method,
				&format!("/api/runs/{}/live-preview{suffix}", Uuid::nil()),
				Value::Null,
			)
			.await;
			assert_eq!(status, expected, "{method} {suffix}: {body}");
		}
	}
}

#[rstest::rstest]
#[tokio::test]
async fn live_preview_stop_authority_belongs_to_the_viewer(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = capability_fixture.await;
	let run = admit(&c).await;
	let (area, _) = working_area(&c, run.id).await;
	let preview = Uuid::new_v4();
	insert_running(&c, preview, run.id, area, 1, "running").await;
	let credential = common::issued_credential(&c.f.store, "acme", "bob").await;
	let bob = credential["token"].as_str().unwrap();
	c.policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"no-run-control","effect":"deny","subjects":{"ids":["alice","bob"]},
		"actions":["run.control"],"resources":{"kinds":["run"]}
	}));
	common::replace_policy(&c.f.store, "acme", c.policy.clone()).await;
	let path = format!("/api/runs/{}/live-preview", run.id);
	let (status, body) = request(&c.app, &c.token, "GET", &path, Value::Null).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		body["may_stop"], true,
		"the requester may stop without run.control"
	);
	// The Area belongs to Alice, so Bob cannot read even its status. A Run's
	// cancellation authority also requires this same visibility decision.
	let (status, body) = request(&c.app, bob, "GET", &path, Value::Null).await;
	assert_eq!(status, 404, "{body}");
	let stop = format!("{path}/stop");
	let (status, body) = request(&c.app, bob, "POST", &stop, Value::Null).await;
	assert_eq!(status, 404, "{body}");
	assert_eq!(live_row(&c, preview).await.0, "running");
	let (status, body) = request(&c.app, &c.token, "POST", &stop, Value::Null).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["live_preview"]["state"], "stopped");
}

#[rstest::rstest]
#[tokio::test]
async fn a_deployment_without_a_preview_domain_issues_no_live_ticket(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let (status, body) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/previews/live-tickets",
		json!({"live_preview_id": Uuid::nil()}),
	)
	.await;
	assert_eq!(status, 404, "{body}");
	let run = admit(&c).await;
	let (area, _) = working_area(&c, run.id).await;
	let preview = Uuid::new_v4();
	insert_running(&c, preview, run.id, area, 1, "running").await;
	let (status, body) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/previews/live-tickets",
		json!({"live_preview_id": preview}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(body["error"]["code"], "PREVIEW_UNAVAILABLE");
}

#[rstest::rstest]
#[tokio::test]
async fn invalid_live_preview_inputs_use_the_house_json_contract(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let limits =
		json!({"tenant":"acme","idle_seconds":600,"absolute_seconds":3600,"expected_revision":0});
	let ticket = json!({"live_preview_id":Uuid::nil()});
	for (method, path, valid) in [
		("POST", "/api/previews/live-tickets", ticket),
		("PUT", "/api/preview-limits/acme", limits),
	] {
		let mut unknown = valid.clone();
		unknown["unexpected"] = json!(true);
		for input in [json!({}), unknown] {
			let (status, body) = request(&c.app, &c.token, method, path, input).await;
			assert_eq!(status, 422, "{method} {path}: {body}");
		}
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_lowers_its_limits_within_the_deployment_ceilings(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let admin = common::BrowserMapping::new(&c.f, "acme", "alice").await;
	let (status, body) = admin
		.send("GET", "/api/preview-limits/acme", Value::Null)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["idle_seconds"], 900);
	assert_eq!(body["absolute_seconds"], 7200);
	assert_eq!(body["ceiling_idle_seconds"], 900);
	assert_eq!(body["ceiling_absolute_seconds"], 7200);
	let (status, body) = admin
		.send(
			"PUT",
			"/api/preview-limits/acme",
			json!({"tenant":"acme","idle_seconds":600,"absolute_seconds":3600,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["idle_seconds"], 600);
	assert_eq!(body["absolute_seconds"], 3600);
	assert_eq!(body["revision"], 1);
	let (status, body) = admin
		.send(
			"PUT",
			"/api/preview-limits/acme",
			json!({"tenant":"acme","idle_seconds":3600,"absolute_seconds":7200,"expected_revision":1}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		body["idle_seconds"], 900,
		"a Tenant cannot raise the Deployment's idle ceiling"
	);
	assert_eq!(body["absolute_seconds"], 7200);
	let (status, body) = admin
		.send(
			"PUT",
			"/api/preview-limits/acme",
			json!({"tenant":"acme","idle_seconds":600,"absolute_seconds":3600,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 409, "{body}");
}

#[rstest::rstest]
#[tokio::test]
async fn cancelling_the_run_stops_its_live_preview(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (area, _digest) = working_area(&c, run.id).await;
	let preview = Uuid::new_v4();
	insert_running(&c, preview, run.id, area, 1, "running").await;
	let (status, body) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/control", run.id),
		json!({"action":"cancel"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (state, reason, confirmed) = live_row(&c, preview).await;
	assert_eq!(state, "stopped");
	assert_eq!(reason.as_deref(), Some("run_cancelled"));
	assert!(!confirmed, "the Pod proof is still owed");
}

#[rstest::rstest]
#[tokio::test]
async fn a_newer_revision_marks_a_running_preview_stale(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (area, digest) = working_area(&c, run.id).await;
	let preview = Uuid::new_v4();
	insert_running(&c, preview, run.id, area, 1, "running").await;
	let (status, body) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"preconditions":{"index.html":digest},"patch":"*** Begin Patch\n*** Update File: index.html\n@@\n-<html>live</html>\n+<html>newer</html>\n*** End Patch"}),
	)
	.await;
	assert_eq!(
		status, 200,
		"a later publication advances the revision: {body}"
	);
	let (status, body) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/live-preview", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["live_preview"]["state"], "stale");
	assert_eq!(body["live_preview"]["revision"], 1);
	assert_eq!(body["live_preview"]["stop_reason"], Value::Null);
	// The stored state keeps running: stale is a visible state, not a stop.
	let (state, reason, _) = live_row(&c, preview).await;
	assert_eq!(state, "running");
	assert_eq!(reason, None);
}

/// Measurement for #781, not a regression test: a Live preview whose start
/// waits for the Runner while its Run is cancelled. A stub Runner accepts the
/// dispatch, reports the Pod as starting until the test releases it, and then
/// reports it running. Observations are printed as `#781` lines.
#[rstest::rstest]
#[tokio::test]
async fn m6_a_live_preview_start_waiting_for_the_runner_when_the_run_is_cancelled(
	#[future] capability_fixture: CoreFixture,
) {
	use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
	let case = "M6 Live preview start racing the cancel";
	let note =
		|what: &str, value: &dyn std::fmt::Debug| eprintln!("#781 {case} | {what} | {value:?}");
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (_area, _) = working_area(&c, run.id).await;
	let run = c.f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	// The stub Runner.
	let released = Arc::new(AtomicBool::new(false));
	let dispatched = Arc::new(AtomicUsize::new(0));
	let cancels = Arc::new(AtomicUsize::new(0));
	let router = {
		let (released, dispatched, cancels) =
			(released.clone(), dispatched.clone(), cancels.clone());
		let cancelled = Arc::new(AtomicBool::new(false));
		let seen = cancelled.clone();
		reinhardt::test::stub::StubRouter::new()
			.post("/v1/operations", move |_request| {
				let dispatched = dispatched.clone();
				async move {
					dispatched.fetch_add(1, Ordering::SeqCst);
					Ok(reinhardt::Response::ok()
						.with_json(&json!({"status":"accepted"}))
						.unwrap())
				}
			})
			.get("/v1/operations/{id}", move |_request| {
				let (released, cancelled) = (released.clone(), seen.clone());
				async move {
					let gone = cancelled.load(Ordering::SeqCst);
					let serve = if gone {
						json!({"status":"stopped","reason":"cancelled"})
					} else if released.load(Ordering::SeqCst) {
						json!({"status":"running","pod_host":"10.0.0.9","startup_log":"serving"})
					} else {
						json!({"status":"starting","startup_log":""})
					};
					Ok(reinhardt::Response::ok()
						.with_json(&json!({"status": if gone {"cancelled"} else {"running"},
							"serve": serve, "termination_confirmed": gone}))
						.unwrap())
				}
			})
			.post("/v1/operations/{id}/cancel", move |_request| {
				let (cancels, cancelled) = (cancels.clone(), cancelled.clone());
				async move {
					cancels.fetch_add(1, Ordering::SeqCst);
					cancelled.store(true, Ordering::SeqCst);
					Ok(reinhardt::Response::ok()
						.with_json(&json!({"status":"cancelled"}))
						.unwrap())
				}
			})
			.into_server_router()
	};
	let runner = reinhardt::test::fixtures::server::test_server_guard(router).await;
	// A store that reaches the stub Runner and has a preview domain. The
	// application keeps its own store, so its cancel marks rows as it would.
	let mut store = c.f.store.clone();
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.runner = Some(
		serde_json::from_value(json!({
			"endpoint": runner.url, "token_env": "AIDASH_781_RUNNER_TOKEN",
			"image": "stub", "runtime_class": "stub", "namespace": "stub",
		}))
		.unwrap(),
	);
	store.capabilities = aidash_server::capabilities::Runtime::new(profile).unwrap();
	store.preview_domain = Some("preview.example.test".into());
	let authority =
		aidash_server::authorization::services::execution::StepAuthority::begin(&c.f, &run)
			.await
			.unwrap();
	let start = {
		let (store, run) = (store.clone(), run.clone());
		tokio::spawn(async move {
			authority
				.live_preview_start(&store, &run, "python3 -m http.server 3000", 3000)
				.await
		})
	};
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
	while dispatched.load(Ordering::SeqCst) == 0 {
		assert!(
			!start.is_finished(),
			"the start ended before its dispatch: {:?}",
			start.await
		);
		assert!(
			tokio::time::Instant::now() < deadline,
			"no dispatch reached the stub Runner"
		);
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}
	let row: (Uuid, String) =
		sqlx::query_as("SELECT id, state FROM core_live_previews WHERE run_id = $1")
			.bind(run.id)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	note("row while the start waits for the Runner", &row);
	let (status, body) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/control", run.id),
		json!({"action":"cancel"}),
	)
	.await;
	note("cancel status", &(status, &body["control"]));
	note(
		"row after the cancel (state, stop_reason, confirmed)",
		&live_row(&c, row.0).await,
	);
	released.store(true, Ordering::SeqCst);
	let started = start.await.unwrap();
	note(
		"live_preview_start result",
		&started
			.as_ref()
			.map(|value| value["state"].clone())
			.map_err(|error| error.to_string()),
	);
	note(
		"row after the start finished (state, stop_reason, confirmed)",
		&live_row(&c, row.0).await,
	);
	let (status, body) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/live-preview", run.id),
		Value::Null,
	)
	.await;
	note(
		"GET live-preview: status, state, stop_reason",
		&(
			status,
			&body["live_preview"]["state"],
			&body["live_preview"]["stop_reason"],
		),
	);
	let (_, run_body) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}", run.id),
		Value::Null,
	)
	.await;
	note(
		"Run phase, control",
		&(&run_body["phase"], &run_body["control"]),
	);
	tokio::time::sleep(std::time::Duration::from_secs(3)).await;
	note(
		"Runner cancel requests within 3 s",
		&cancels.load(Ordering::SeqCst),
	);
	let holds: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM core_live_previews WHERE run_id = $1 AND state <> 'stopped'",
	)
	.bind(run.id)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	note("rows of the cancelled Run not stopped", &holds);
	drop(runner);
	c.close().await;
}
