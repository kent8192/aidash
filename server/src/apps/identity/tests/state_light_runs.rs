#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
//! Run lists expose aggregates, while the detail endpoint retains context.
use serde_json::{Value, json};
use uuid::Uuid;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;

#[rstest::rstest]
#[tokio::test]
async fn all_run_lists_are_light_and_detail_still_has_context(
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings { actor_burst: 100_000, ..Default::default() })]
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
	let (status, created) = common::request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let id: Uuid = sqlx::query_scalar("SELECT id FROM runs WHERE task_id = $1")
		.bind(task)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (status, detail) =
		common::request(&app, &token, "GET", &format!("/api/runs/{id}"), Value::Null).await;
	assert_eq!(status, 200, "{detail}");
	let full = &detail["run"];
	assert!(full["context"].is_object(), "{detail}");
	for path in [
		"/api/state".to_owned(),
		format!("/api/tasks/{task}/runs"),
		format!("/api/workspaces/{workspace}/runs"),
	] {
		let (status, response) = common::request(&app, &token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{path}: {response}");
		let rows = if path == "/api/state" {
			&response["runs"]
		} else {
			&response["items"]
		};
		let run = rows
			.as_array()
			.unwrap()
			.iter()
			.find(|row| row["id"] == id.to_string())
			.unwrap();
		assert!(run.get("context").is_none(), "{path}: {run}");
		for field in ["id", "state", "recovery", "state_version"] {
			assert_eq!(run[field], full[field], "{path}: {field}");
		}
		assert_eq!(
			run["input_tokens"],
			full["context"]["usage"]["input_tokens"]
		);
		assert_eq!(
			run["output_tokens"],
			full["context"]["usage"]["output_tokens"]
		);
		let calls = full["context"]["history"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|entry| entry["event"]["kind"] == "tool")
			.count();
		assert_eq!(run["tool_calls"], calls);
	}
	common::cleanup(f, &url, &schema).await;
}
