//! The dashboard read does not add SQL round trips for each Run in its window.
use serde_json::{Value, json};
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
#[path = "../../execution/tests/support/statements.rs"]
mod statements;

#[rstest::rstest]
#[tokio::test]
async fn state_statement_count_does_not_grow_with_runs(
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings { actor_burst: 100_000, ..Default::default() })]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, first_task) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let mut few = 0;
	for index in 0..30 {
		let task = if index == 0 {
			first_task.to_string()
		} else {
			let (status, workspace) = common::request(
				&app,
				&token,
				"POST",
				"/api/workspaces",
				json!({"title":"Measured", "goal":"Bounded state"}),
			)
			.await;
			assert_eq!(status, 200, "{workspace}");
			let workspace = workspace["id"].as_str().unwrap();
			let (status, task) = common::request(
				&app,
				&token,
				"POST",
				&format!("/api/workspaces/{workspace}/tasks"),
				json!({"title":"Measured task", "description":"Create a run"}),
			)
			.await;
			assert_eq!(status, 200, "{task}");
			task["id"].as_str().unwrap().to_owned()
		};
		let (status, run) = common::request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
		)
		.await;
		assert_eq!(status, 200, "{run}");
		if index == 2 || index == 29 {
			let (status, body) =
				common::request(&app, &token, "GET", "/api/state", Value::Null).await;
			assert_eq!(status, 200, "{body}");
			let ((status, body), statements) = statements::recorded(common::request(
				&app,
				&token,
				"GET",
				"/api/state",
				Value::Null,
			))
			.await;
			assert_eq!(status, 200, "{body}");
			assert!(!statements.is_empty());
			assert_eq!(body["runs"].as_array().unwrap().len(), index + 1);
			println!(
				"STATE_STATEMENTS runs={} count={}",
				index + 1,
				statements.len()
			);
			if index == 2 {
				few = statements.len();
			} else {
				assert_eq!(
					few,
					statements.len(),
					"few={few}, many={}",
					statements.len()
				);
			}
		}
	}
	common::cleanup(f, &url, &schema).await;
}
