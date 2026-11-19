//! Duties (#1011, ADR 0142; from the Schedules of #557, ADR 0134):
//! registration, the time triggers every replica runs, the Requester losing
//! access, the Duty's own Thread and the Agent tools.
use super::*;
use aidash_server::apps::workspaces::services::duties::{agent_tool, pass};
use chrono::{Duration as Span, Utc};

fn definition(title: &str, every_seconds: i64) -> Value {
	json!({
		"title": title,
		"mission": "Summarize what changed since the last tick.",
		"agent": {"id": "research", "version": "1.1.0"},
		"trigger": {"kind": "interval", "every_seconds": every_seconds},
	})
}

async fn register(c: &CoreFixture, workspace: Uuid, body: Value) -> Value {
	let (status, duty) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/duties"),
		body,
	)
	.await;
	assert_eq!(status, 200, "{duty}");
	duty
}

/// Moves a Duty's next tick into the past, as if its time had come. An
/// interval counts from its anchor, which moves with it, so every call is a
/// new scheduled time.
async fn make_due(c: &CoreFixture, duty: Uuid) {
	let due = Utc::now() - Span::seconds(90);
	sqlx::query(
		"UPDATE duties SET anchor = $1 - interval '1 day', next_scheduled_at = $1, next_fire_at = $1 WHERE id = $2",
	)
		.bind(due)
		.bind(duty)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
}

async fn ticks(c: &CoreFixture, duty: Uuid) -> Vec<(String, Option<String>, Option<Uuid>)> {
	sqlx::query_as(
		"SELECT status, reason, task_id FROM duty_ticks WHERE duty_id = $1 ORDER BY recorded_at",
	)
	.bind(duty)
	.fetch_all(c.f.store.pool.fixture_driver())
	.await
	.unwrap()
}

async fn halt(c: &CoreFixture, duty: Uuid) -> Option<String> {
	sqlx::query_scalar("SELECT halt FROM duties WHERE id = $1")
		.bind(duty)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

async fn workspace(c: &CoreFixture) -> Uuid {
	c.f.store
		.task(&TEST_FIXTURE, c.task)
		.await
		.unwrap()
		.workspace_id
}

#[rstest::rstest]
#[tokio::test]
async fn a_tick_starts_one_run_as_its_requester_across_two_schedulers(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let duty = register(&c, workspace, definition("Hourly digest", 3600)).await;
	let id: Uuid = serde_json::from_value(duty["id"].clone()).unwrap();
	assert_eq!(duty["mine"], true);
	assert_eq!(duty["status"], "active");
	assert!(duty["next_scheduled_at"].is_string());
	make_due(&c, id).await;
	// Act: two replicas run their scheduler pass at the same moment.
	let now = Utc::now();
	let (first, second) = tokio::join!(pass(&c.f, now), pass(&c.f, now));
	// Assert: the tick started exactly one Task, recorded once.
	let started = first.unwrap().started.len() + second.unwrap().started.len();
	assert_eq!(started, 1);
	let recorded = ticks(&c, id).await;
	assert_eq!(recorded.len(), 1, "{recorded:?}");
	assert_eq!(recorded[0].0, "started");
	let task =
		c.f.store
			.task(&TEST_FIXTURE, recorded[0].2.unwrap())
			.await
			.unwrap();
	assert_eq!(task.title, "Hourly digest");
	let root: String = sqlx::query_scalar(
		"SELECT e.root_subject FROM authorization_execution e JOIN runs r ON r.id = e.run_id WHERE r.task_id = $1",
	)
	.bind(task.id)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(json!(root), duty["requester"]);
	let next: chrono::DateTime<Utc> =
		sqlx::query_scalar("SELECT next_scheduled_at FROM duties WHERE id = $1")
			.bind(id)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert!(next > now);
	// A second pass finds nothing due.
	assert_eq!(pass(&c.f, Utc::now()).await.unwrap().started.len(), 0);
	// The next tick arrives while that Run is unfinished: it is skipped.
	make_due(&c, id).await;
	let overlap = pass(&c.f, Utc::now()).await.unwrap();
	assert_eq!(overlap.skipped, vec![(id, "overlap".to_owned())]);
	assert_eq!(ticks(&c, id).await[1].0, "skipped");
	// The history and the bell name the Task the Duty started.
	let (status, detail) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/duties/{id}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{detail}");
	assert_eq!(detail["ticks"].as_array().unwrap().len(), 2);
	let (status, notices) =
		request(&c.app, &c.token, "GET", "/api/duties/notices", Value::Null).await;
	assert_eq!(status, 200, "{notices}");
	assert_eq!(notices["scheduled_tasks"], json!([task.id]));
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_duty_stops_when_its_requester_loses_access(#[future] capability_fixture: CoreFixture) {
	// Arrange: the Requester's credential is revoked, as leaving does.
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let duty = register(&c, workspace, definition("Daily digest", 86_400)).await;
	let id: Uuid = serde_json::from_value(duty["id"].clone()).unwrap();
	make_due(&c, id).await;
	sqlx::query(
		"UPDATE authorization_credentials SET revoked_at = now() WHERE id = (SELECT credential_id FROM duties WHERE id = $1)",
	)
	.bind(id)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	// Act
	let report = pass(&c.f, Utc::now()).await.unwrap();
	// Assert: nothing ran in their name; the Duty waits for a successor.
	assert!(report.started.is_empty());
	assert_eq!(halt(&c, id).await.as_deref(), Some("requester_left"));
	c.close().await;
}

/// There is no minimum interval and no count limit but a plan's (#1012).
#[rstest::rstest]
#[tokio::test]
async fn any_frequency_and_any_number_of_duties_are_accepted(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	// Act
	for (title, every) in [
		("Every second", 1),
		("Every minute", 60),
		("Every hour", 3600),
	] {
		register(&c, workspace, definition(title, every)).await;
	}
	let (status, limits) =
		request(&c.app, &c.token, "GET", "/api/duties/limits", Value::Null).await;
	// Assert: every one registered, and the limits route is gone.
	let (listed_status, listed) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/workspaces/{workspace}/duties"),
		Value::Null,
	)
	.await;
	assert_eq!(listed_status, 200, "{listed}");
	assert_eq!(listed.as_array().unwrap().len(), 3);
	assert_ne!(status, 200, "{limits}");
	c.close().await;
}

/// The Duty's own Thread: its first Task opens it, every later Task works in
/// it, and deleting it stops the Duty (#1011).
async fn duty_thread(c: &CoreFixture, duty: Uuid) -> Option<Uuid> {
	sqlx::query_scalar("SELECT thread_id FROM duties WHERE id = $1")
		.bind(duty)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

/// Ends a Duty's open Task so its next tick is not skipped as an overlap.
async fn finish_task(c: &CoreFixture, task: Uuid) {
	sqlx::query("UPDATE tasks SET status = 'COMPLETED' WHERE id = $1")
		.bind(task)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn a_duty_works_in_its_own_thread_and_stops_when_it_is_deleted(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: a Duty whose first two Tasks ran.
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let duty = register(&c, workspace, definition("Weekly review", 604_800)).await;
	let id: Uuid = serde_json::from_value(duty["id"].clone()).unwrap();
	assert_eq!(duty["thread_id"], Value::Null);
	make_due(&c, id).await;
	let first = pass(&c.f, Utc::now()).await.unwrap();
	assert_eq!(first.started.len(), 1, "{first:?}");
	let thread = duty_thread(&c, id)
		.await
		.expect("the first Task opens the Thread");
	finish_task(&c, first.started[0]).await;
	make_due(&c, id).await;
	let second = pass(&c.f, Utc::now()).await.unwrap();
	assert_eq!(second.started.len(), 1, "{second:?}");
	let threads: Vec<Option<Uuid>> =
		sqlx::query_scalar("SELECT thread_id FROM duty_ticks WHERE duty_id = $1")
			.bind(id)
			.fetch_all(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(threads, vec![Some(thread), Some(thread)]);
	// The Tasks left Working Areas in the Thread; the person keeps them.
	let areas: Vec<(Uuid, i64)> =
		sqlx::query_as("SELECT id, revision FROM core_areas WHERE thread_id = $1")
			.bind(thread)
			.fetch_all(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	let files: Vec<Value> = areas
		.iter()
		.map(
			|(area, revision)| json!({"area_id": area, "expected_revision": revision, "choice": "keep"}),
		)
		.collect();
	// Act
	let (status, deleted) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads/{thread}/delete"),
		json!({"idempotency_key": Uuid::new_v4(), "files": files}),
	)
	.await;
	// Assert
	assert_eq!(status, 200, "{deleted}");
	assert_eq!(halt(&c, id).await.as_deref(), Some("thread_deleted"));
	let (status, notices) =
		request(&c.app, &c.token, "GET", "/api/duties/notices", Value::Null).await;
	assert_eq!(status, 200, "{notices}");
	assert_eq!(notices["stopped"][0]["halt"], "thread_deleted");
	c.close().await;
}

/// The mode a Duty's Thread holds for the fixture's person, if any.
async fn thread_mode(c: &CoreFixture, thread: &Value) -> Option<String> {
	sqlx::query_scalar("SELECT mode FROM identity_thread_approval_modes WHERE thread_id = $1")
		.bind(Uuid::parse_str(thread["id"].as_str().unwrap()).unwrap())
		.fetch_optional(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn a_duty_set_back_to_the_default_mode_clears_its_threads_mode(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: a Duty whose first Task ran in its own mode.
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let mut body = definition("Standup notes", 86_400);
	body["approval_mode"] = json!("dont_ask");
	let duty = register(&c, workspace, body).await;
	let id: Uuid = serde_json::from_value(duty["id"].clone()).unwrap();
	make_due(&c, id).await;
	let first = pass(&c.f, Utc::now()).await.unwrap();
	assert_eq!(first.started.len(), 1, "{first:?}");
	let thread = json!({"id": duty_thread(&c, id).await.unwrap()});
	assert_eq!(thread_mode(&c, &thread).await.as_deref(), Some("dont_ask"));
	sqlx::query("UPDATE tasks SET status = 'COMPLETED' WHERE id = $1")
		.bind(first.started[0])
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	sqlx::query(
		"UPDATE duty_ticks SET scheduled_at = scheduled_at - interval '3 days' WHERE duty_id = $1",
	)
	.bind(id)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	// Act: the Requester returns it to their own default mode.
	let (status, changed) = request(
		&c.app,
		&c.token,
		"PATCH",
		&format!("/api/duties/{id}"),
		json!({"approval_mode": "default"}),
	)
	.await;
	assert_eq!(status, 200, "{changed}");
	make_due(&c, id).await;
	let second = pass(&c.f, Utc::now()).await.unwrap();
	// Assert: the next tick runs in the person's default, not the old mode.
	assert_eq!(second.started.len(), 1, "{second:?}");
	assert_eq!(thread_mode(&c, &thread).await, None);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_agent_registers_for_its_person_and_so_may_a_run_a_duty_started(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: an ordinary Run of the person, and a Run a Duty started.
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let input = json!({
		"title": "Agent digest",
		"mission": "Collect the open questions.",
		"trigger": {"kind": "cron", "expression": "0 9 * * 1-5", "time_zone": "Asia/Tokyo"},
	});
	// Act
	let created = agent_tool(&c.f.store, &run, "call-1", "duty_create", input.clone())
		.await
		.unwrap();
	let listed = agent_tool(&c.f.store, &run, "call-2", "duty_list", json!({}))
		.await
		.unwrap();
	// Assert: it acts as the person at the root of the Run's chain.
	assert_eq!(created["acts_as_you"], true);
	assert_eq!(listed["duties"][0]["title"], "Agent digest");
	let id: Uuid = serde_json::from_value(created["duty_id"].clone()).unwrap();
	let (registered_by, agent): (Option<Uuid>, String) =
		sqlx::query_as("SELECT created_by_run_id, agent_id FROM duties WHERE id = $1")
			.bind(id)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(registered_by, Some(run.id));
	assert_eq!(agent, "research");
	// The person's Duty in another Workspace is out of the Agent's reach.
	let (status, other) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/workspaces",
		json!({"title": "Elsewhere", "goal": "Another Workspace"}),
	)
	.await;
	assert_eq!(status, 200, "{other}");
	let elsewhere: Uuid = serde_json::from_value(
		other["id"]
			.clone()
			.as_str()
			.map(|id| json!(id))
			.unwrap_or_else(|| other["workspace"]["id"].clone()),
	)
	.unwrap();
	let foreign = register(&c, elsewhere, definition("Elsewhere", 86_400)).await;
	let reach = agent_tool(
		&c.f.store,
		&run,
		"call-3",
		"duty_delete",
		json!({"duty_id": foreign["id"], "action": "pause"}),
	)
	.await;
	assert!(
		matches!(&reach, Err(aidash_server::Error::Invalid(_))),
		"{reach:?}"
	);
	// A Run a Duty started may set up Duties too (#1012).
	make_due(&c, id).await;
	let report = pass(&c.f, Utc::now()).await.unwrap();
	assert_eq!(report.started.len(), 1, "{report:?}");
	let scheduled =
		c.f.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|candidate| candidate.task_id == report.started[0])
			.unwrap();
	let mut again = input;
	again["title"] = json!("Set up by a Duty");
	let created = agent_tool(&c.f.store, &scheduled, "call-4", "duty_create", again).await;
	assert!(
		created
			.as_ref()
			.is_ok_and(|created| created["acts_as_you"] == true),
		"{created:?}"
	);
	assert!(
		agent_tool(&c.f.store, &scheduled, "call-5", "duty_list", json!({}))
			.await
			.is_ok()
	);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_refused_start_is_a_failed_tick_with_its_reason_and_is_not_retried(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: the scheduled Agent's model loses its Catalog Approval (#540).
	// The Agent itself stays approved, so the tick passes the Duty's own
	// checks and the Run start refuses it.
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let duty = register(&c, workspace, definition("Hourly digest", 3600)).await;
	let id: Uuid = serde_json::from_value(duty["id"].clone()).unwrap();
	let agent =
		c.f.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"research",
				"1.1.0",
			)
			.await
			.unwrap();
	let model = &agent.config["model"];
	let (model_id, model_version) = (
		model["id"].as_str().unwrap(),
		model["version"].as_str().unwrap(),
	);
	let withdrawn = sqlx::query(
		"UPDATE authorization_catalog SET enabled = false WHERE tenant = 'acme' AND entry_id = $1 AND entry_version = $2",
	)
	.bind(model_id)
	.bind(model_version)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(withdrawn.rows_affected() > 0);
	make_due(&c, id).await;
	// Act
	let now = Utc::now();
	let report = pass(&c.f, now).await.unwrap();
	// Assert: recorded as failed, with its reason, and moved on.
	assert!(report.started.is_empty(), "{report:?}");
	assert_eq!(report.failed.len(), 1, "{report:?}");
	let recorded = ticks(&c, id).await;
	assert_eq!(recorded.len(), 1, "{recorded:?}");
	assert_eq!(recorded[0].0, "failed");
	assert!(
		recorded[0]
			.1
			.as_deref()
			.is_some_and(|reason| reason == aidash_server::Error::Forbidden.to_string()),
		"{recorded:?}"
	);
	let (next, failures): (chrono::DateTime<Utc>, i32) =
		sqlx::query_as("SELECT next_scheduled_at, consecutive_failures FROM duties WHERE id = $1")
			.bind(id)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert!(next > now);
	assert_eq!(failures, 1);
	// A second pass does not retry the refused tick.
	pass(&c.f, Utc::now()).await.unwrap();
	assert_eq!(ticks(&c, id).await.len(), 1);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn three_failed_runs_in_a_row_pause_a_duty_and_the_bell_says_so(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: a Duty with its own Approval Mode.
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let mut body = definition("Nightly check", 86_400);
	body["approval_mode"] = json!("dont_ask");
	let duty = register(&c, workspace, body).await;
	let id: Uuid = serde_json::from_value(duty["id"].clone()).unwrap();
	for round in 0..3 {
		// Earlier ticks lie far enough back that the next is not too soon.
		sqlx::query(
			"UPDATE duty_ticks SET scheduled_at = scheduled_at - interval '3 days' WHERE duty_id = $1",
		)
		.bind(id)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		make_due(&c, id).await;
		// Act: the tick starts a Run, which fails; the next pass settles it.
		let report = pass(&c.f, Utc::now()).await.unwrap();
		assert_eq!(report.started.len(), 1, "round {round}: {report:?}");
		let task = report.started[0];
		if round == 0 {
			// The Duty's mode is its Thread's mode for the Requester.
			let mode: String = sqlx::query_scalar(
				"SELECT m.mode FROM identity_thread_approval_modes m JOIN duty_ticks k ON k.thread_id = m.thread_id WHERE k.task_id = $1",
			)
			.bind(task)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
			assert_eq!(mode, "dont_ask");
		}
		sqlx::query("UPDATE tasks SET status = 'FAILED' WHERE id = $1")
			.bind(task)
			.execute(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
		let settled = pass(&c.f, Utc::now()).await.unwrap();
		assert_eq!(settled.settled, 1, "round {round}: {settled:?}");
	}
	// Assert: paused for failures, and the Requester is told.
	assert_eq!(halt(&c, id).await.as_deref(), Some("failures"));
	let statuses: Vec<String> = ticks(&c, id).await.into_iter().map(|tick| tick.0).collect();
	assert_eq!(statuses, ["failed", "failed", "failed"]);
	let (status, notices) =
		request(&c.app, &c.token, "GET", "/api/duties/notices", Value::Null).await;
	assert_eq!(status, 200, "{notices}");
	assert_eq!(notices["stopped"][0]["halt"], "failures");
	// Only the Requester resumes it, and it runs again.
	let (status, resumed) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/duties/{id}/resume"),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{resumed}");
	assert_eq!(resumed["status"], "active");
	assert_eq!(resumed["consecutive_failures"], 0);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_agent_definition_removes_the_duty_tools(#[future] capability_fixture: CoreFixture) {
	// Arrange: a new version of the Agent without the `duties` capability.
	let c = capability_fixture.await;
	let (status, mut agent) = request(
		&c.app,
		&c.token,
		"GET",
		"/api/registry/research/1.1.0",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{agent}");
	agent
		.as_object_mut()
		.unwrap()
		.remove("binding_normalization");
	let tools = ["duty_create", "duty_list", "duty_update", "duty_delete"];
	let mut duplicated = agent.clone();
	agent["version"] = json!("1.2.0");
	agent["config"]["remove_default"] = json!(tools);
	duplicated["version"] = json!("1.3.0");
	duplicated["config"]["remove_default"] = json!(["duty_list", "duty_list"]);
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	// Act
	let saved =
		c.f.registry
			.register(&tenant, serde_json::from_value(agent).unwrap())
			.await;
	let refused =
		c.f.registry
			.register(&tenant, serde_json::from_value(duplicated).unwrap())
			.await;
	// Assert: accepted, and none of the tools is bound; a repeated name is refused.
	assert!(saved.is_ok(), "{saved:?}");
	let stored =
		c.f.registry
			.get(&tenant, "research", "1.2.0")
			.await
			.unwrap();
	let normalized = serde_json::to_string(&stored.binding_normalization).unwrap();
	for tool in tools {
		assert!(
			!normalized.contains(&format!("aidash.{tool}")),
			"{tool}: {normalized}"
		);
	}
	assert!(refused.is_err(), "{refused:?}");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_retried_duty_tool_call_returns_the_duty_it_registered(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let input = json!({
		"title": "Retried digest",
		"mission": "Collect the open questions.",
		"trigger": {"kind": "interval", "every_seconds": 3600},
	});
	// Act: the same tool call arrives twice, as after a lost answer.
	let first = agent_tool(&c.f.store, &run, "call-a", "duty_create", input.clone())
		.await
		.unwrap();
	let again = agent_tool(&c.f.store, &run, "call-a", "duty_create", input.clone())
		.await
		.unwrap();
	let other = agent_tool(&c.f.store, &run, "call-b", "duty_create", input)
		.await
		.unwrap();
	// Assert
	assert_eq!(again["duty_id"], first["duty_id"]);
	assert_ne!(other["duty_id"], first["duty_id"]);
	let registered: i64 =
		sqlx::query_scalar("SELECT count(*) FROM duties WHERE title = 'Retried digest'")
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(registered, 2);
	c.close().await;
}
