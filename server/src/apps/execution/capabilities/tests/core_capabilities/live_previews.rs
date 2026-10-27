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
async fn stopping_a_missing_run_is_forbidden(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let (status, body) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/live-preview/stop", Uuid::nil()),
		Value::Null,
	)
	.await;
	assert_eq!(status, 403, "{body}");
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
	let (status, credential) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(status, 200, "{credential}");
	let bob = credential["token"].as_str().unwrap();
	c.policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"no-run-control","effect":"deny","subjects":{"ids":["alice","bob"]},
		"actions":["run.control"],"resources":{"kinds":["run"]}
	}));
	common::replace_policy(&c.f.store, "acme", c.policy.clone()).await;
	let path = format!("/api/runs/{}/live-preview", run.id);
	for (token, allowed) in [(c.token.as_str(), true), (bob, false)] {
		let (status, body) = request(&c.app, token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{body}");
		assert_eq!(
			body["may_stop"], allowed,
			"requester exception uses the viewer's own identity"
		);
	}
	let stop = format!("{path}/stop");
	let (status, body) = request(&c.app, bob, "POST", &stop, Value::Null).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(live_row(&c, preview).await.0, "running");
	// Restoring Bob's own cancellation authority enables Stop without borrowing
	// the requester's execution grant or changing the preview's owner.
	c.policy["policies"].as_array_mut().unwrap().pop();
	common::replace_policy(&c.f.store, "acme", c.policy.clone()).await;
	let (status, body) = request(&c.app, bob, "GET", &path, Value::Null).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["may_stop"], true);
	let (status, body) = request(&c.app, bob, "POST", &stop, Value::Null).await;
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
	let admin = &c.f.config.api_token;
	let (status, body) = request(
		&c.app,
		admin,
		"GET",
		"/api/preview-limits/acme",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["idle_seconds"], 900);
	assert_eq!(body["absolute_seconds"], 7200);
	assert_eq!(body["ceiling_idle_seconds"], 900);
	assert_eq!(body["ceiling_absolute_seconds"], 7200);
	let (status, body) = request(
		&c.app,
		admin,
		"PUT",
		"/api/preview-limits/acme",
		json!({"tenant":"acme","idle_seconds":600,"absolute_seconds":3600,"expected_revision":0}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["idle_seconds"], 600);
	assert_eq!(body["absolute_seconds"], 3600);
	assert_eq!(body["revision"], 1);
	let (status, body) = request(
		&c.app,
		admin,
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
	let (status, body) = request(
		&c.app,
		admin,
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
