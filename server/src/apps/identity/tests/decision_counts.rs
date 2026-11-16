//! How many `registry.read` decisions a read walk and a worker boundary write (#404, #769).
//!
//! The counts are the same on the commit before #769: a definition is decided once per
//! transaction however many Runs of a walk name it.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::harness::Harness;
use serde_json::{Value, json};
use uuid::Uuid;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;

/// The `registry.read` decisions written so far, one count per resource.
async fn registry_reads(f: &aidash_server::federation::Federation) -> Vec<(String, i64)> {
	sqlx::query_as(
		"SELECT resource_id, COUNT(*) FROM authorization_decisions \
		 WHERE action = 'registry.read' GROUP BY resource_id ORDER BY resource_id",
	)
	.fetch_all(f.store.pool.fixture_driver())
	.await
	.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn workspace_live_runs_continue_to_an_older_paused_run(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, task) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = $1")
		.bind(task)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (status, body) = common::request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let old: Uuid = sqlx::query_scalar("SELECT id FROM runs WHERE task_id = $1")
		.bind(task)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
	sqlx::query(
		"UPDATE runs SET control = 'PAUSED', updated_at = now() - interval '30 days' WHERE id = $1",
	)
	.bind(old)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	for _ in 0..50 {
		let (status, task) = common::request(
			&app,
			&token,
			"POST",
			&format!("/api/workspaces/{workspace}/tasks"),
			json!({"title":"New task", "description":"New run"}),
		)
		.await;
		assert_eq!(status, 200, "{task}");
		let (status, run) = common::request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{}/claim", task["id"].as_str().unwrap()),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
		)
		.await;
		assert_eq!(status, 200, "{run}");
	}
	let (status, first) = common::request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/runs?live=true"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{first}");
	assert_eq!(first["items"].as_array().unwrap().len(), 50);
	let cursor = first["next_cursor"].as_str().unwrap();
	let (status, second) = common::request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/runs?live=true&cursor={cursor}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{second}");
	assert!(
		second["items"]
			.as_array()
			.unwrap()
			.iter()
			.any(|run| run["id"] == old.to_string() && run["control"] == "PAUSED")
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_walk_over_two_runs_with_one_binding_decides_each_definition_once(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, first) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	// A second Task of the same Workspace, claimed by the same Agent, so both Runs name one Binding.
	let run = |task: Uuid| {
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}).to_string()
			+ &task.to_string()
	};
	let _ = run;
	let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = $1")
		.bind(first)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (status, second) = common::request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Second","description":"Same Binding"}),
	)
	.await;
	assert_eq!(status, 200, "{second}");
	let second: Uuid = second["id"].as_str().unwrap().parse().unwrap();
	for task in [first, second] {
		let (status, body) = common::request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	let runs = f.store.runs(&TEST_FIXTURE).await.unwrap();
	assert_eq!(runs.len(), 2);
	let (read, reader) = (runs[0].id, runs[1].id);
	// The second Run read the first, so reading the second walks both.
	sqlx::query(
		"INSERT INTO authorization_run_reads (run_id, tenant, workspace_id, resource_kind, resource_id) \
		 VALUES ($1, 'acme', $2, 'run', $3)",
	)
	.bind(reader)
	.bind(workspace)
	.bind(read)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();

	let before = registry_reads(&f).await;
	let (status, body) = common::request(
		&app,
		&token,
		"GET",
		&format!("/api/runs/{reader}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let after = registry_reads(&f).await;
	let written: Vec<(String, i64)> = after
		.iter()
		.map(|(id, count)| {
			let earlier = before
				.iter()
				.find(|(other, _)| other == id)
				.map_or(0, |(_, count)| *count);
			(id.clone(), count - earlier)
		})
		.filter(|(_, count)| *count > 0)
		.collect();
	assert!(!written.is_empty(), "the walk decided nothing");
	assert!(
		written.iter().all(|(_, count)| *count == 1),
		"each definition is decided once across both Runs: {written:?}"
	);
	println!(
		"WALK_DEFINITIONS={} WALK_DECISIONS={}",
		written.len(),
		written.iter().map(|(_, c)| c).sum::<i64>()
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn one_worker_boundary_writes_the_registry_read_rows_it_wrote_before(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, task) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, body) = common::request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let before = registry_reads(&f).await;
	let _ = Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await;
	let after = registry_reads(&f).await;
	let written: Vec<(String, i64)> = after
		.iter()
		.map(|(id, count)| {
			let earlier = before
				.iter()
				.find(|(other, _)| other == id)
				.map_or(0, |(_, count)| *count);
			(id.clone(), count - earlier)
		})
		.filter(|(_, count)| *count > 0)
		.collect();
	let total: i64 = written.iter().map(|(_, count)| count).sum();
	println!("BOUNDARY_DECISIONS={total} PER_RESOURCE={written:?}");
	// 48 on the commit before #769 (46 after: the Decider is decided once, not twice).
	assert!(
		total <= 48,
		"a boundary writes no more than before: {total} {written:?}"
	);
	assert!(total > 0, "the boundary decided nothing");
	common::cleanup(f, &url, &schema).await;
}

/// A larger Tenant must not increase the decision cost of a dashboard read.
#[rstest::rstest]
#[tokio::test]
async fn state_decisions_stay_bounded_when_workspaces_and_runs_double(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, first_task) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;

	let (status, first_run) = common::request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{first_task}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{first_run}");

	let run_id: Uuid = sqlx::query_scalar("SELECT id FROM runs WHERE task_id = $1")
		.bind(first_task)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = $1")
		.bind(first_task)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let request_id = Uuid::new_v4();
	sqlx::query("INSERT INTO human_requests (id, tenant, workspace_id, run_id, kind, prompt, request_key, created_at) VALUES ($1, 'acme', $2, $3, 'QUESTION', 'Still waiting?', 'old-request', now() - interval '30 days')")
        .bind(request_id).bind(workspace).bind(run_id).execute(f.store.pool.fixture_driver()).await.unwrap();
	let deadline = chrono::Utc::now() + chrono::Duration::days(2);
	let waiting = aidash_domain::RunState::Waiting(Box::new(aidash_domain::WaitingState::Human {
		request_id,
		answer_by: Some(deadline),
		resume: aidash_domain::ResumeState::Ready(aidash_domain::ReadyState {}),
	}));
	let pending = aidash_domain::run_state::encode(&waiting, &Default::default()).unwrap();
	sqlx::query("UPDATE runs SET phase = 'WAITING', pending = $1, created_at = now() - interval '30 days', updated_at = now() - interval '30 days' WHERE id = $2")
		.bind(pending).bind(run_id).execute(f.store.pool.fixture_driver()).await.unwrap();
	let count = || async {
		sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM authorization_decisions")
			.fetch_one(f.store.pool.fixture_driver())
			.await
			.unwrap()
	};
	let mut measurements = Vec::new();
	let mut decision_breakdowns = Vec::new();
	for batch in [59, 60] {
		for _ in 0..batch {
			let (status, workspace) = common::request(
				&app,
				&token,
				"POST",
				"/api/workspaces",
				json!({"title":"Window", "goal":"Bounded state"}),
			)
			.await;
			assert_eq!(status, 200, "{workspace}");
			let workspace = workspace["id"].as_str().unwrap();
			if !measurements.is_empty() {
				// Add older history while keeping the current windows identical.
				sqlx::query(
					"UPDATE workspaces SET created_at = created_at - interval '1 year' WHERE id = $1",
				)
				.bind(Uuid::parse_str(workspace).unwrap())
				.execute(f.store.pool.fixture_driver())
				.await
				.unwrap();
			}
			let (status, task) = common::request(
				&app,
				&token,
				"POST",
				&format!("/api/workspaces/{workspace}/tasks"),
				json!({"title":"Window task", "description":"Create a run"}),
			)
			.await;
			assert_eq!(status, 200, "{task}");
			let (status, run) = common::request(
				&app,
				&token,
				"POST",
				&format!("/api/tasks/{}/claim", task["id"].as_str().unwrap()),
				json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
			)
			.await;
			assert_eq!(status, 200, "{run}");
		}
		// Keep the newest Task and Run windows aligned with the newest Workspace
		// window: otherwise scheduler updates can change how many distinct
		// Workspaces the windows happen to reference between the measurements.
		sqlx::query("UPDATE tasks SET created_at = workspaces.created_at FROM workspaces WHERE tasks.workspace_id = workspaces.id AND tasks.tenant = 'acme'")
			.execute(f.store.pool.fixture_driver()).await.unwrap();
		sqlx::query("UPDATE runs SET updated_at = tasks.created_at FROM tasks WHERE runs.task_id = tasks.id AND runs.tenant = 'acme' AND runs.id <> $1")
			.bind(run_id).execute(f.store.pool.fixture_driver()).await.unwrap();
		// Hold the independent event projection constant while changing the sizes
		// of the Workspace and Run collections.
		sqlx::query("DELETE FROM events WHERE tenant = 'acme'")
			.execute(f.store.pool.fixture_driver())
			.await
			.unwrap();
		// Registry admission has its own caches and is outside this change. Warm
		// them for both measurements so the first read is not a cold start.
		let (status, warm) = common::request(&app, &token, "GET", "/api/state", Value::Null).await;
		assert_eq!(status, 200, "{warm}");
		let before = count().await;
		let sequence: i64 =
			sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM authorization_decisions")
				.fetch_one(f.store.pool.fixture_driver())
				.await
				.unwrap();
		let (status, state) = common::request(&app, &token, "GET", "/api/state", Value::Null).await;
		assert_eq!(status, 200, "{state}");

		assert!(
			state["human_requests"]
				.as_array()
				.unwrap()
				.iter()
				.any(|row| row["id"] == request_id.to_string()),
			"old pending request missing: {state}"
		);
		let old_request = state["human_requests"]
			.as_array()
			.unwrap()
			.iter()
			.find(|row| row["id"] == request_id.to_string())
			.unwrap();
		assert_eq!(
			old_request["answer_by"]
				.as_str()
				.unwrap()
				.parse::<chrono::DateTime<chrono::Utc>>()
				.unwrap(),
			deadline
		);
		assert!(
			!state["runs"]
				.as_array()
				.unwrap()
				.iter()
				.any(|row| row["id"] == run_id.to_string()),
			"old run is still inside the window"
		);
		let after = count().await;
		decision_breakdowns.push(sqlx::query_as::<_, (String, i64)>("SELECT action, COUNT(*) FROM authorization_decisions WHERE sequence > $1 GROUP BY action ORDER BY action")
			.bind(sequence).fetch_all(f.store.pool.fixture_driver()).await.unwrap());
		assert!(state["next_cursors"]["workspaces"].is_string());
		assert!(state["next_cursors"]["runs"].is_string());
		measurements.push(after - before);
		let cursor = state["next_cursors"]["workspaces"].as_str().unwrap();
		let node = state["workspaces"][0]["node"].as_str().unwrap();
		let (status, page) = common::request(
			&app,
			&token,
			"GET",
			&format!("/api/workspaces?node={node}&cursor={cursor}"),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{page}");
		let workspaces = page["items"].as_array().unwrap();
		assert!(!workspaces.is_empty());
		assert!(workspaces.len() <= 50);
		assert!(
			workspaces
				.iter()
				.all(|row| row["node"] == node && row["id"] != cursor)
		);
		assert_eq!(page["next_cursor"].is_string(), workspaces.len() == 50);
	}
	assert_eq!(
		measurements[0], measurements[1],
		"decision counts: {measurements:?}; by action: {decision_breakdowns:?}"
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn state_node_scope_empty_cursors_and_administrator_collections(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, _) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, state) = common::request(&app, &token, "GET", "/api/state", Value::Null).await;
	assert_eq!(status, 200, "{state}");
	let workspace = &state["workspaces"][0];
	let node = workspace["node"].as_str().unwrap();
	assert!(state["next_cursors"]["workspaces"].is_null());
	let (status, scoped) = common::request(
		&app,
		&token,
		"GET",
		&format!("/api/state?node={node}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{scoped}");
	assert!(
		scoped["workspaces"]
			.as_array()
			.unwrap()
			.iter()
			.all(|row| row["node"] == node)
	);
	assert!(
		scoped["events"]
			.as_array()
			.unwrap()
			.iter()
			.all(|row| row["node"].is_null() || row["node"] == node)
	);
	assert!(
		scoped["workspaces"]
			.as_array()
			.unwrap()
			.iter()
			.any(|row| row["id"] == workspace["id"])
	);
	let (status, absent) = common::request(
		&app,
		&token,
		"GET",
		"/api/state?node=missing-node",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{absent}");
	for field in [
		"workspaces",
		"tasks",
		"runs",
		"artifacts",
		"human_requests",
		"conversations",
	] {
		assert!(
			absent[field].as_array().unwrap().is_empty(),
			"{field}: {absent}"
		);
		assert!(absent["next_cursors"][field].is_null());
	}
	let (status, admin) =
		common::request(&app, &f.config.api_token, "GET", "/api/state", Value::Null).await;
	assert_eq!(status, 200, "{admin}");
	for field in [
		"workspaces",
		"tasks",
		"runs",
		"artifacts",
		"human_requests",
		"conversations",
	] {
		assert!(admin[field].as_array().unwrap().is_empty());
		assert!(admin["next_cursors"][field].is_null());
	}
	common::cleanup(f, &url, &schema).await;
}
