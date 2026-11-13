//! Schedules (#557, ADR 0134): registration, the scheduler every replica runs,
//! the Registrant losing access, limits, Thread deletion and the Agent tools.
use super::*;
use aidash_server::apps::workspaces::services::schedules::{agent_tool, pass};
use chrono::{Duration as Span, Utc};

fn definition(title: &str, every_seconds: i64) -> Value {
	json!({
		"title": title,
		"request": "Summarize what changed since the last tick.",
		"agent": {"id": "research", "version": "1.1.0"},
		"trigger": {"kind": "interval", "every_seconds": every_seconds},
	})
}

async fn register(c: &CoreFixture, workspace: Uuid, body: Value) -> Value {
	let (status, schedule) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/schedules"),
		body,
	)
	.await;
	assert_eq!(status, 200, "{schedule}");
	schedule
}

/// Moves a Schedule's next tick into the past, as if its time had come. An
/// interval counts from its anchor, which moves with it, so every call is a
/// new scheduled time.
async fn make_due(c: &CoreFixture, schedule: Uuid) {
	let due = Utc::now() - Span::seconds(90);
	sqlx::query(
		"UPDATE schedules SET anchor = $1 - interval '1 day', next_scheduled_at = $1, next_fire_at = $1 WHERE id = $2",
	)
		.bind(due)
		.bind(schedule)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
}

async fn ticks(c: &CoreFixture, schedule: Uuid) -> Vec<(String, Option<String>, Option<Uuid>)> {
	sqlx::query_as(
		"SELECT status, reason, task_id FROM schedule_ticks WHERE schedule_id = $1 ORDER BY recorded_at",
	)
	.bind(schedule)
	.fetch_all(c.f.store.pool.fixture_driver())
	.await
	.unwrap()
}

async fn halt(c: &CoreFixture, schedule: Uuid) -> Option<String> {
	sqlx::query_scalar("SELECT halt FROM schedules WHERE id = $1")
		.bind(schedule)
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
async fn a_tick_starts_one_run_as_its_registrant_across_two_schedulers(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let schedule = register(&c, workspace, definition("Hourly digest", 3600)).await;
	let id: Uuid = serde_json::from_value(schedule["id"].clone()).unwrap();
	assert_eq!(schedule["mine"], true);
	assert_eq!(schedule["status"], "active");
	assert!(schedule["next_scheduled_at"].is_string());
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
	assert_eq!(json!(root), schedule["registrant"]);
	let next: chrono::DateTime<Utc> =
		sqlx::query_scalar("SELECT next_scheduled_at FROM schedules WHERE id = $1")
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
	// The history and the bell name the Task the Schedule started.
	let (status, detail) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/schedules/{id}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{detail}");
	assert_eq!(detail["ticks"].as_array().unwrap().len(), 2);
	let (status, notices) = request(
		&c.app,
		&c.token,
		"GET",
		"/api/schedules/notices",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{notices}");
	assert_eq!(notices["scheduled_tasks"], json!([task.id]));
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_schedule_stops_when_its_registrant_loses_access(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: the Registrant's credential is revoked, as leaving does.
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let schedule = register(&c, workspace, definition("Daily digest", 86_400)).await;
	let id: Uuid = serde_json::from_value(schedule["id"].clone()).unwrap();
	make_due(&c, id).await;
	sqlx::query(
		"UPDATE authorization_credentials SET revoked_at = now() WHERE id = (SELECT credential_id FROM schedules WHERE id = $1)",
	)
	.bind(id)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	// Act
	let report = pass(&c.f, Utc::now()).await.unwrap();
	// Assert: nothing ran in their name; the Schedule waits for a successor.
	assert!(report.started.is_empty());
	assert_eq!(halt(&c, id).await.as_deref(), Some("registrant_left"));
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_tenant_limits_bound_registration(#[future] capability_fixture: CoreFixture) {
	// Arrange
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let (status, limits) = request(
		&c.app,
		&c.token,
		"PUT",
		"/api/schedules/limits",
		json!({"max_schedules": 50, "max_per_person": 1, "min_interval_seconds": 1800}),
	)
	.await;
	assert_eq!(status, 200, "{limits}");
	// Act
	let (too_frequent, _) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/schedules"),
		definition("Every 15 minutes", 900),
	)
	.await;
	register(&c, workspace, definition("Every hour", 3600)).await;
	let (over, refused) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/schedules"),
		definition("Another", 3600),
	)
	.await;
	let (below_floor, _) = request(
		&c.app,
		&c.token,
		"PUT",
		"/api/schedules/limits",
		json!({"max_schedules": 50, "max_per_person": 1, "min_interval_seconds": 60}),
	)
	.await;
	// Assert
	assert_eq!(too_frequent, 400);
	assert_eq!(over, 409, "{refused}");
	assert_eq!(below_floor, 400);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deleting_a_schedules_fixed_thread_stops_it(#[future] capability_fixture: CoreFixture) {
	// Arrange
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let root = source(&c, workspace, "Weekly review").await;
	let (status, thread) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id": root}),
	)
	.await;
	assert_eq!(status, 200, "{thread}");
	let mut body = definition("Weekly review", 604_800);
	body["thread"] = json!({"kind": "fixed", "thread_id": thread["id"]});
	let schedule = register(&c, workspace, body).await;
	let id: Uuid = serde_json::from_value(schedule["id"].clone()).unwrap();
	// Act
	let (status, deleted) = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/workspaces/{workspace}/threads/{}/delete",
			thread["id"].as_str().unwrap()
		),
		json!({"idempotency_key": Uuid::new_v4(), "files": []}),
	)
	.await;
	// Assert
	assert_eq!(status, 200, "{deleted}");
	assert_eq!(halt(&c, id).await.as_deref(), Some("thread_deleted"));
	let (status, notices) = request(
		&c.app,
		&c.token,
		"GET",
		"/api/schedules/notices",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{notices}");
	assert_eq!(notices["stopped"][0]["halt"], "thread_deleted");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_agent_registers_for_its_person_and_a_scheduled_run_cannot(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: an ordinary Run of the person, and a Run a Schedule started.
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let input = json!({
		"title": "Agent digest",
		"request": "Collect the open questions.",
		"trigger": {"kind": "cron", "expression": "0 9 * * 1-5", "time_zone": "Asia/Tokyo"},
	});
	// Act
	let created = agent_tool(&c.f.store, &run, "call-1", "schedule_create", input.clone())
		.await
		.unwrap();
	let listed = agent_tool(&c.f.store, &run, "call-2", "schedule_list", json!({}))
		.await
		.unwrap();
	// Assert: it acts as the person at the root of the Run's chain.
	assert_eq!(created["acts_as_you"], true);
	assert_eq!(listed["schedules"][0]["title"], "Agent digest");
	let id: Uuid = serde_json::from_value(created["schedule_id"].clone()).unwrap();
	let (registered_by, agent): (Option<Uuid>, String) =
		sqlx::query_as("SELECT created_by_run_id, agent_id FROM schedules WHERE id = $1")
			.bind(id)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(registered_by, Some(run.id));
	assert_eq!(agent, "research");
	// The person's Schedule in another Workspace is out of the Agent's reach.
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
		"schedule_delete",
		json!({"schedule_id": foreign["id"], "action": "pause"}),
	)
	.await;
	assert!(
		matches!(&reach, Err(aidash_server::Error::Invalid(_))),
		"{reach:?}"
	);
	// A tick's Run may list Schedules but not register or change them.
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
	let refused = agent_tool(&c.f.store, &scheduled, "call-4", "schedule_create", input).await;
	assert!(
		matches!(&refused, Err(aidash_server::Error::Invalid(message)) if message.contains("started by a Schedule")),
		"{refused:?}"
	);
	assert!(
		agent_tool(&c.f.store, &scheduled, "call-5", "schedule_list", json!({}))
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
	use aidash_domain::registry::bindings::{COMPACTION_DECIDER_ID, COMPACTION_DECIDER_VERSION};
	// Arrange: the Agent's compaction Decider loses its Catalog Approval (#540).
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let schedule = register(&c, workspace, definition("Hourly digest", 3600)).await;
	let id: Uuid = serde_json::from_value(schedule["id"].clone()).unwrap();
	let withdrawn = sqlx::query(
		"UPDATE authorization_catalog SET enabled = false WHERE tenant = 'acme' AND entry_id = $1 AND entry_version = $2",
	)
	.bind(COMPACTION_DECIDER_ID)
	.bind(COMPACTION_DECIDER_VERSION)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(withdrawn.rows_affected() > 0);
	make_due(&c, id).await;
	// Act
	let now = Utc::now();
	let report = pass(&c.f, now).await.unwrap();
	// Assert: recorded as failed, naming the Decider, and moved on.
	assert!(report.started.is_empty(), "{report:?}");
	assert_eq!(report.failed.len(), 1, "{report:?}");
	let recorded = ticks(&c, id).await;
	assert_eq!(recorded.len(), 1, "{recorded:?}");
	assert_eq!(recorded[0].0, "failed");
	assert!(
		recorded[0]
			.1
			.as_deref()
			.is_some_and(|reason| reason.contains("COMPACTION_DECIDER_UNAPPROVED")),
		"{recorded:?}"
	);
	let (next, failures): (chrono::DateTime<Utc>, i32) = sqlx::query_as(
		"SELECT next_scheduled_at, consecutive_failures FROM schedules WHERE id = $1",
	)
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
async fn three_failed_runs_in_a_row_pause_a_schedule_and_the_bell_says_so(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: a Schedule with its own Approval Mode.
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let mut body = definition("Nightly check", 86_400);
	body["approval_mode"] = json!("dont_ask");
	let schedule = register(&c, workspace, body).await;
	let id: Uuid = serde_json::from_value(schedule["id"].clone()).unwrap();
	for round in 0..3 {
		// Earlier ticks lie far enough back that the next is not too soon.
		sqlx::query(
			"UPDATE schedule_ticks SET scheduled_at = scheduled_at - interval '3 days' WHERE schedule_id = $1",
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
			// The Schedule's mode is its Thread's mode for the Registrant.
			let mode: String = sqlx::query_scalar(
				"SELECT m.mode FROM identity_thread_approval_modes m JOIN schedule_ticks k ON k.thread_id = m.thread_id WHERE k.task_id = $1",
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
	// Assert: paused for failures, and the Registrant is told.
	assert_eq!(halt(&c, id).await.as_deref(), Some("failures"));
	let statuses: Vec<String> = ticks(&c, id).await.into_iter().map(|tick| tick.0).collect();
	assert_eq!(statuses, ["failed", "failed", "failed"]);
	let (status, notices) = request(
		&c.app,
		&c.token,
		"GET",
		"/api/schedules/notices",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{notices}");
	assert_eq!(notices["stopped"][0]["halt"], "failures");
	// Only the Registrant resumes it, and it runs again.
	let (status, resumed) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/schedules/{id}/resume"),
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
async fn an_agent_definition_removes_the_schedule_tools(#[future] capability_fixture: CoreFixture) {
	// Arrange: a new version of the Agent without the `schedules` capability.
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
	let tools = [
		"schedule_create",
		"schedule_list",
		"schedule_update",
		"schedule_delete",
	];
	let mut duplicated = agent.clone();
	agent["version"] = json!("1.2.0");
	agent["config"]["remove_default"] = json!(tools);
	duplicated["version"] = json!("1.3.0");
	duplicated["config"]["remove_default"] = json!(["schedule_list", "schedule_list"]);
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
async fn a_retried_schedule_tool_call_returns_the_schedule_it_registered(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let input = json!({
		"title": "Retried digest",
		"request": "Collect the open questions.",
		"trigger": {"kind": "interval", "every_seconds": 3600},
	});
	// Act: the same tool call arrives twice, as after a lost answer.
	let first = agent_tool(&c.f.store, &run, "call-a", "schedule_create", input.clone())
		.await
		.unwrap();
	let again = agent_tool(&c.f.store, &run, "call-a", "schedule_create", input.clone())
		.await
		.unwrap();
	let other = agent_tool(&c.f.store, &run, "call-b", "schedule_create", input)
		.await
		.unwrap();
	// Assert
	assert_eq!(again["schedule_id"], first["schedule_id"]);
	assert_ne!(other["schedule_id"], first["schedule_id"]);
	let registered: i64 =
		sqlx::query_scalar("SELECT count(*) FROM schedules WHERE title = 'Retried digest'")
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(registered, 2);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_run_a_schedule_started_cannot_escape_its_mark_by_delegating(
	#[future] capability_fixture: CoreFixture,
) {
	use aidash_server::{
		authorization::services::execution::Step,
		tool::{ToolContext, builtins},
	};
	// Arrange: the Run's Agent may create and delegate Tasks, and a tick
	// starts a Run, which a worker leases.
	let mut c = capability_fixture.await;
	c.policy["policies"].as_array_mut().unwrap().push(json!({
		"id": "agents-delegate", "effect": "allow", "subjects": {"any": true},
		"actions": ["node.use", "workspace.read", "tool.invoke", "task.create", "task.read", "task.delegate", "agent.execute", "registry.read"],
		"resources": {"kinds": ["*"]}
	}));
	crate::common::replace_policy(&c.f.store, "acme", c.policy.clone()).await;
	let workspace = workspace(&c).await;
	let schedule = register(&c, workspace, definition("Hourly digest", 3600)).await;
	let id: Uuid = serde_json::from_value(schedule["id"].clone()).unwrap();
	make_due(&c, id).await;
	let report = pass(&c.f, Utc::now()).await.unwrap();
	assert_eq!(report.started.len(), 1, "{report:?}");
	let marked: bool =
		sqlx::query_scalar("SELECT scheduled FROM authorization_execution WHERE task_id = $1")
			.bind(report.started[0])
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert!(marked, "the tick marks the Run it started");
	let leased =
		c.f.store
			.lease_run(Uuid::new_v4(), 120)
			.await
			.unwrap()
			.unwrap();
	assert_eq!(leased.task_id, report.started[0]);
	// The tools see the Run's own authority, as a worker's step holds it.
	let step = Step::begin(&c.f, &leased).await.unwrap();
	let ctx = ToolContext {
		home: step.home(c.f.clone()),
		store: c.f.store.clone(),
		run: leased.clone(),
		scope: TEST_FIXTURE.clone(),
	};
	let tools = builtins();
	let agent = json!({"id": "research", "version": "1.0.0"});
	// (b) A Task created under the person's own Task, then delegated.
	let child = tools["task_create"]
		.invoke(
			&ctx,
			json!({"title": "Side work", "description": "Outside the chain", "requirements": {}, "parent_id": c.task}),
			"escape-create",
		)
		.await
		.unwrap();
	let child: Uuid = serde_json::from_value(child["id"].clone()).unwrap();
	tools["task_delegate"]
		.invoke(
			&ctx,
			json!({"task_id": child, "node_id": c.f.config.deployment, "agent": agent}),
			"escape-delegate-b",
		)
		.await
		.unwrap();
	// (a) The person's older, unrelated Task, delegated.
	tools["task_delegate"]
		.invoke(
			&ctx,
			json!({"task_id": c.task, "node_id": c.f.config.deployment, "agent": agent}),
			"escape-delegate-a",
		)
		.await
		.unwrap();
	step.commit().await.unwrap();
	let runs = c.f.store.runs(&TEST_FIXTURE).await.unwrap();
	let input = json!({
		"title": "Escaped",
		"request": "Register myself again.",
		"trigger": {"kind": "interval", "every_seconds": 3600},
	});
	// Act + Assert: neither delegated Run may register a Schedule.
	for task in [child, c.task] {
		let delegated = runs
			.iter()
			.find(|run| run.task_id == task)
			.unwrap_or_else(|| panic!("no Run for {task}"));
		let refused = agent_tool(
			&c.f.store,
			delegated,
			"escape-schedule",
			"schedule_create",
			input.clone(),
		)
		.await;
		assert!(
			matches!(&refused, Err(aidash_server::Error::Invalid(message)) if message.contains("started by a Schedule")),
			"{task}: {refused:?}"
		);
	}
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_raised_minimum_interval_stops_a_schedule_and_says_why(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: an hourly Schedule, then the Tenant asks for two hours at least.
	let c = capability_fixture.await;
	let workspace = workspace(&c).await;
	let schedule = register(&c, workspace, definition("Hourly digest", 3600)).await;
	let id: Uuid = serde_json::from_value(schedule["id"].clone()).unwrap();
	let (status, limits) = request(
		&c.app,
		&c.token,
		"PUT",
		"/api/schedules/limits",
		json!({"max_schedules": 50, "max_per_person": 10, "min_interval_seconds": 7200}),
	)
	.await;
	assert_eq!(status, 200, "{limits}");
	assert_eq!(limits["effective"]["min_interval_seconds"], 7200);
	make_due(&c, id).await;
	// Act
	let report = pass(&c.f, Utc::now()).await.unwrap();
	// Assert: stopped with a notice naming the new minimum, not skipped forever.
	assert!(report.started.is_empty(), "{report:?}");
	assert_eq!(halt(&c, id).await.as_deref(), Some("interval_too_short"));
	let recorded = ticks(&c, id).await;
	assert_eq!(recorded.len(), 1, "{recorded:?}");
	assert_eq!(recorded[0].0, "skipped");
	let (status, notices) = request(
		&c.app,
		&c.token,
		"GET",
		"/api/schedules/notices",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{notices}");
	assert_eq!(notices["stopped"][0]["halt"], "interval_too_short");
	assert!(
		notices["stopped"][0]["halt_reason"]
			.as_str()
			.is_some_and(|reason| reason.contains("120 minutes")),
		"{notices}"
	);
	c.close().await;
}
