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
	// 54 with the Schedule tools (#557): 46 plus four default tools, two rows each.
	assert!(
		total <= 54,
		"a boundary writes no more than before: {total} {written:?}"
	);
	assert!(total > 0, "the boundary decided nothing");
	common::cleanup(f, &url, &schema).await;
}
