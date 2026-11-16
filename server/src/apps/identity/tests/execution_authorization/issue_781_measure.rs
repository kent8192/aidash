//! Regression coverage for cancellation while a call asks a person.
//!
//! A call that passed the step's control check holds the Run as it was loaded
//! and its authority. Each case therefore loads the Run and begins its
//! authority while the Run is active, cancels it through the real endpoint, and
//! then makes the write the call would make.
use super::*;

fn note(case: &str, what: &str, value: impl std::fmt::Debug) {
	eprintln!("#781 {case} | {what} | {value:?}");
}

async fn cancel(app: &common::TestApplication, token: &str, run: Uuid) -> (u16, Value) {
	request(
		app,
		token,
		"POST",
		&format!("/api/runs/{run}/control"),
		json!({"action":"cancel"}),
	)
	.await
}

/// What the person sees of `request` on every surface that lists questions.
async fn person_sees(case: &str, app: &common::TestApplication, token: &str, run: Uuid, id: Uuid) {
	let (status, state) = request(app, token, "GET", "/api/state", Value::Null).await;
	let shown: Vec<Value> = state["human_requests"]
		.as_array()
		.cloned()
		.unwrap_or_default()
		.into_iter()
		.filter(|request| request["id"] == json!(id))
		.collect();
	note(case, "GET /api/state status", status);
	note(
		case,
		"GET /api/state lists the request (response null = pending in the dashboard)",
		shown
			.iter()
			.map(|request| request["response"].clone())
			.collect::<Vec<_>>(),
	);
	for query in ["", "?state=pending", &format!("?run={run}&state=pending")] {
		let (status, page) = request(
			app,
			token,
			"GET",
			&format!("/api/human-requests{query}"),
			Value::Null,
		)
		.await;
		let listed = page["items"]
			.as_array()
			.or(page.as_array())
			.map(|items| items.iter().any(|request| request["id"] == json!(id)));
		note(
			case,
			&format!("GET /api/human-requests{query} status, listed"),
			(status, listed),
		);
	}
	let (status, run_body) =
		request(app, token, "GET", &format!("/api/runs/{run}"), Value::Null).await;
	note(
		case,
		"GET /api/runs/{run} status, phase, control",
		(
			status,
			run_body["phase"].clone(),
			run_body["control"].clone(),
		),
	);
}

async fn answer(_case: &str, app: &common::TestApplication, token: &str, id: Uuid) {
	let (status, body) = request(
		app,
		token,
		"POST",
		&format!("/api/human-requests/{id}/answer"),
		json!("an answer after the cancel"),
	)
	.await;
	assert_eq!(status, 409, "closed request answer: {body}");
}

async fn rows(f: &aidash_server::federation::Federation, sql: &str, run: Uuid) -> i64 {
	sqlx::query_scalar(sql)
		.bind(run)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap()
}

/// Baseline: a question asked before the cancel, which the Run waits on.
#[rstest::rstest]
#[tokio::test]
async fn m1_a_question_the_run_waits_on_when_it_is_cancelled(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let case = "M1 question asked before the cancel";
	let t = tool_call_approvals;
	t.question.store(true, Ordering::SeqCst);
	let (f, url, schema) = t.application.runtime.parts();
	let (token, run) = claimed_run(&t, &f, |_| {}).await;
	let app = &t.application.application;
	let harness = Harness {
		federation: f.clone(),
	};
	let waiting = step_until(&harness, &f, run.id, |run| {
		matches!(&run.state, aidash_server::domain::RunState::Waiting(waiting)
			if matches!(waiting.as_ref(), aidash_server::domain::WaitingState::Human { .. }))
	})
	.await;
	let aidash_server::domain::RunState::Waiting(state) = &waiting.state else {
		unreachable!()
	};
	let aidash_server::domain::WaitingState::Human { request_id, .. } = state.as_ref() else {
		unreachable!()
	};
	let id = *request_id;
	person_sees(&format!("{case} / before cancel"), app, &token, run.id, id).await;
	let (status, body) = cancel(app, &token, run.id).await;
	assert_eq!(status, 200, "cancel: {body}");
	let (status, page) = request(
		app,
		&token,
		"GET",
		"/api/human-requests?state=pending",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert!(
		!page["items"]
			.as_array()
			.or(page.as_array())
			.unwrap()
			.iter()
			.any(|item| item["id"] == json!(id))
	);
	let (status, state) = request(app, &token, "GET", "/api/state", Value::Null).await;
	assert_eq!(status, 200);
	let closed = state["human_requests"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == json!(id))
		.unwrap();
	assert_eq!(closed["answered_by"], "system");
	assert_eq!(closed["response"]["reason"], "run_ended");
	person_sees(&format!("{case} / after cancel"), app, &token, run.id, id).await;
	for _ in 0..3 {
		harness.worker_once().await.unwrap();
	}
	person_sees(
		&format!("{case} / after worker steps"),
		app,
		&token,
		run.id,
		id,
	)
	.await;
	answer(case, app, &token, id).await;
	let current = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	note(
		case,
		"Run after answer: phase, control",
		(current.phase().as_str(), current.control),
	);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}

/// A question written after the cancel committed, by a call that passed the
/// step's control check before it.
#[rstest::rstest]
#[tokio::test]
async fn m2_a_question_written_after_the_cancel(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let case = "M2 question written after the cancel";
	let t = tool_call_approvals;
	let (f, url, schema) = t.application.runtime.parts();
	let (token, run) = claimed_run(&t, &f, |_| {}).await;
	let app = &t.application.application;
	let authority = StepAuthority::begin(&f, &run).await;
	note(case, "authority begun while active", authority.is_ok());
	let (status, body) = cancel(app, &token, run.id).await;
	assert_eq!(status, 200, "cancel: {body}");
	let asked = f
		.store
		.human_request(
			&TEST_FIXTURE,
			&run,
			"QUESTION",
			"Late question",
			"late-question",
		)
		.await;
	assert!(
		matches!(&asked, Err(aidash_server::Error::Conflict(message)) if message == "run has ended"),
		"late question: {asked:?}"
	);
	note(
		case,
		"store.human_request result",
		asked
			.as_ref()
			.map(|request| request.id)
			.map_err(|error| error.to_string()),
	);
	assert_eq!(
		rows(
			&f,
			"SELECT count(*) FROM human_requests WHERE run_id = $1",
			run.id
		)
		.await,
		0
	);
	let current = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	note(
		case,
		"Run: phase, control",
		(current.phase().as_str(), current.control),
	);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}

/// A sampling approval opened after the cancel committed, through the same
/// authority a step holds. It shares the approval cards of Tool call and
/// disclosure approvals.
#[rstest::rstest]
#[tokio::test]
async fn m3_a_sampling_approval_opened_after_the_cancel(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let case = "M3 sampling approval after the cancel";
	let t = tool_call_approvals;
	let (f, url, schema) = t.application.runtime.parts();
	let (token, run) = claimed_run(&t, &f, |_| {}).await;
	let app = &t.application.application;
	let authority = StepAuthority::begin(&f, &run).await.unwrap();
	let (status, body) = cancel(app, &token, run.id).await;
	assert_eq!(status, 200, "cancel: {body}");
	let request = aidash_application::capabilities::approvals::SamplingApprovalRequest {
		key: "late-sampling".into(),
		target: serde_json::from_value(json!({
			"agent": run.agent_id,
			"plugin": "late-plugin",
			"server": "late-server",
			"digest": format!("sha256:{}", "0".repeat(64)),
			"origin": "https://example.com",
		}))
		.unwrap(),
		request: serde_json::from_value(json!({
			"messages": [{"role": "user", "text": "Late question"}],
			"system_prompt": "",
			"max_tokens": 16,
		}))
		.unwrap(),
		expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
		area: None,
	};
	let asked = authority.sampling_approval(&f.store, &run, request).await;
	note(case, "sampling_approval result", format!("{asked:?}"));
	note(
		case,
		"core_records sampling rows",
		rows(
			&f,
			"SELECT count(*) FROM core_records WHERE kind = 'sampling' AND $1::uuid IS NOT NULL",
			run.id,
		)
		.await,
	);
	let pending = approval_cards(app, &token, "?state=pending&role=approver").await;
	note(case, "pending inbox (approver) cards", pending.len());
	let all = approval_cards(app, &token, "").await;
	assert!(pending.is_empty());
	assert!(all.iter().any(|card| card["kind"] == "sampling"
		&& card["state"] == "closed"
		&& card["closed_reason"] == "run_ended"));
	note(
		case,
		"all cards: kind, state, closed_reason",
		all.iter()
			.map(|card| {
				(
					card["kind"].clone(),
					card["state"].clone(),
					card["closed_reason"].clone(),
				)
			})
			.collect::<Vec<_>>(),
	);
	let current = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	note(
		case,
		"Run: phase, control",
		(current.phase().as_str(), current.control),
	);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}
