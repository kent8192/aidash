#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
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
			assert!(statements.len() <= 100, "{} statements", statements.len());
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

#[rstest::rstest]
#[tokio::test]
async fn state_statement_count_is_bounded_with_denied_provenance_and_human_request(
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings { actor_burst: 100_000, ..Default::default() })]
	fixture: common::ApplicationFixture,
) {
	use uuid::Uuid;

	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut bundle, token, first_task) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let db = f.store.pool.fixture_driver();
	let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = $1")
		.bind(first_task)
		.fetch_one(db)
		.await
		.unwrap();
	let artifacts = [Uuid::new_v4(), Uuid::new_v4()];
	for id in artifacts {
		sqlx::query("INSERT INTO artifacts (id, tenant, workspace_id, task_id, kind, name, content, created_by, idempotency_key) VALUES ($1, 'acme', $2, $3, 'json', 'Source', '{}', 'alice', $1::text)")
			.bind(id).bind(workspace).bind(first_task).execute(db).await.unwrap();
	}
	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":"deny-source", "effect":"deny", "subjects":{"ids":["alice"]},
		"actions":["artifact.read"], "resources":{"kinds":["artifact"],"ids":[artifacts[1]]}
	}));
	let (status, body) = common::request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":bundle}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut few = 0;
	let mut hidden = Vec::new();
	let mut visible = Vec::new();
	for index in 0..30 {
		let task = if index == 0 {
			first_task
		} else {
			let (status, task) = common::request(
				&app,
				&token,
				"POST",
				&format!("/api/workspaces/{workspace}/tasks"),
				json!({"title":"Measured task", "description":"Create a run"}),
			)
			.await;
			assert_eq!(status, 200, "{task}");
			task["id"].as_str().unwrap().parse().unwrap()
		};
		let (status, body) = common::request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		let run: Uuid = sqlx::query_scalar("SELECT id FROM runs WHERE task_id = $1")
			.bind(task)
			.fetch_one(db)
			.await
			.unwrap();
		let is_hidden = index % 2 == 1;
		if is_hidden {
			hidden.push(run);
		} else {
			visible.push(run);
		}
		sqlx::query("INSERT INTO authorization_run_reads (run_id, tenant, workspace_id, resource_kind, resource_id) VALUES ($1, 'acme', $2, 'artifact', $3)")
			.bind(run).bind(workspace).bind(artifacts[usize::from(is_hidden)]).execute(db).await.unwrap();
		if index == 0 {
			let request = Uuid::new_v4();
			sqlx::query("INSERT INTO human_requests (id, tenant, workspace_id, run_id, kind, prompt, request_key, created_at) VALUES ($1, 'acme', $2, $3, 'QUESTION', 'Still waiting?', 'measure-mixed', now())")
				.bind(request).bind(workspace).bind(run).execute(db).await.unwrap();
			let state =
				aidash_domain::RunState::Waiting(Box::new(aidash_domain::WaitingState::Human {
					request_id: request,
					answer_by: None,
					resume: aidash_domain::ResumeState::Ready(aidash_domain::ReadyState {}),
				}));
			let pending = aidash_domain::run_state::encode(&state, &Default::default()).unwrap();
			sqlx::query("UPDATE runs SET phase = 'WAITING', pending = $1 WHERE id = $2")
				.bind(pending)
				.bind(run)
				.execute(db)
				.await
				.unwrap();
		}
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
			let shown = body["runs"].as_array().unwrap();
			for id in &hidden {
				assert!(
					!shown.iter().any(|row| row["id"] == id.to_string()),
					"hidden run {id}: {body}"
				);
			}
			for id in &visible {
				assert!(
					shown.iter().any(|row| row["id"] == id.to_string()),
					"visible run {id}: {body}"
				);
			}
			assert_eq!(
				body["human_requests"].as_array().unwrap().len(),
				1,
				"{body}"
			);
			if std::env::var_os("AIDASH_STATE_MEASURE_SQL").is_some() {
				for statement in &statements {
					println!("MIXED_SQL {statement}");
				}
			}
			assert!(statements.len() <= 100, "{} statements", statements.len());
			println!(
				"MIXED_STATE_STATEMENTS runs={} count={}",
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
