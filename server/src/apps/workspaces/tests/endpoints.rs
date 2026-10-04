use crate::endpoint::{EndpointFixture, assert_json, endpoint, workspace};
use aidash_server::apps::workspaces::models::{Task, TaskDependency, Workspace};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn workspace_round_trip_and_revision_conflict_preserve_saved_state(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	let created = workspace(&app.operator, "Workspace over HTTP").await;
	let id: Uuid = serde_json::from_value(created["id"].clone()).unwrap();
	let path = format!("/api/workspaces/{id}");
	// Act
	let updated = app
		.operator
		.patch(&path, &json!({"revision":0,"state":{"step":1}}), "json")
		.await
		.unwrap();
	let stale = app
		.operator
		.patch(&path, &json!({"revision":0,"state":{"step":2}}), "json")
		.await
		.unwrap();
	let snapshot = app.operator.get(&path).await.unwrap();
	// Assert
	assert_eq!(assert_json(updated, 200)["revision"], 1);
	assert_json(stale, 409);
	assert_eq!(
		assert_json(snapshot, 200)["workspace"]["state"],
		json!({"step":1})
	);
	let saved = Workspace::objects()
		.filter(Workspace::field_id().eq(id))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(saved.revision, 1);
	assert_eq!(*saved.state, json!({"step":1}));
}

#[rstest]
#[tokio::test]
async fn task_dependencies_are_projected_once_and_cycles_are_rejected_over_http(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	let workspace = workspace(&app.operator, "Task graph").await;
	let workspace_id = workspace["id"].as_str().unwrap();
	let path = format!("/api/workspaces/{workspace_id}/tasks");
	let ancestor = assert_json(
		app.operator
			.post(
				&path,
				&json!({"title":"Ancestor","description":"Root"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let dependent_input = json!({"title":"Dependent","description":"Wait for ancestor","dependencies":[ancestor["id"],ancestor["id"]]});
	let body = dependent_input.to_string();
	let headers = [("idempotency-key", "endpoint-task")];
	// Act
	let dependent = assert_json(
		app.operator
			.post_raw_with_headers(&path, body.as_bytes(), "application/json", &headers)
			.await
			.unwrap(),
		200,
	);
	let repeated = app
		.operator
		.post_raw_with_headers(&path, body.as_bytes(), "application/json", &headers)
		.await
		.unwrap();
	let cycle = app.operator.post(&path, &json!({"title":"Cycle","description":"Reject","parent_id":ancestor["id"],"dependencies":[dependent["id"]]}), "json").await.unwrap();
	// Assert
	assert_eq!(assert_json(repeated, 200)["id"], dependent["id"]);
	// The development baseline enforces combined cycles with a DB trigger.
	// Keep its public error envelope while checking the atomic rollback below.
	assert_eq!(
		assert_json(cycle, 500)["error"],
		"operation failed; see server logs"
	);
	let id = serde_json::from_value::<Uuid>(dependent["id"].clone()).unwrap();
	let links = TaskDependency::objects()
		.filter(TaskDependency::field_task_key().eq(id))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(links.len(), 1);
	let snapshot = assert_json(
		app.operator
			.get(&format!("/api/workspaces/{workspace_id}"))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(snapshot["tasks"].as_array().unwrap().len(), 2);
}

#[rstest]
#[tokio::test]
async fn invalid_json_and_path_parameters_do_not_create_records(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	// Act
	let malformed = app
		.operator
		.post_raw("/api/workspaces", b"{", "application/json")
		.await
		.unwrap();
	let unknown = app
		.operator
		.post(
			"/api/workspaces",
			&json!({"title":"Rejected","goal":"Contract","unexpected":true}),
			"json",
		)
		.await
		.unwrap();
	let invalid_id = app
		.operator
		.get("/api/workspaces/not-a-uuid")
		.await
		.unwrap();
	// Assert
	assert_eq!(malformed.status_code(), 400, "{}", malformed.text());
	assert_eq!(unknown.status_code(), 422, "{}", unknown.text());
	assert_eq!(invalid_id.status_code(), 400, "{}", invalid_id.text());
	assert!(
		Workspace::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
}

#[rstest]
#[tokio::test]
async fn workspace_and_task_content_reject_unicode_blank_strings_over_http(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let home = workspace(&app.operator, "Validation boundary").await;
	let path = format!("/api/workspaces/{}/tasks", home["id"].as_str().unwrap());
	let whitespace: Vec<_> = (0..=0x10ffff)
		.filter_map(char::from_u32)
		.filter(|c| c.is_whitespace())
		.collect();
	let mut blanks: Vec<String> = whitespace.iter().map(char::to_string).collect();
	blanks.extend([whitespace.iter().collect(), String::new()]);
	for blank in blanks {
		for field in ["title", "goal"] {
			let mut input = json!({"title":"Workspace", "goal":"Goal"});
			input[field] = json!(blank);
			let response = app
				.operator
				.post("/api/workspaces", &input, "json")
				.await
				.unwrap();
			assert_eq!(
				response.status_code(),
				400,
				"{field}={blank:?}: {}",
				response.text()
			);
		}
		for field in ["title", "description"] {
			let mut input = json!({"title":"Task", "description":"Work"});
			input[field] = json!(blank);
			let response = app.operator.post(&path, &input, "json").await.unwrap();
			assert_eq!(
				response.status_code(),
				400,
				"{field}={blank:?}: {}",
				response.text()
			);
		}
	}
	assert_eq!(
		Workspace::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.len(),
		1
	);
	assert!(
		Task::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	for content in ["\t\u{a0}content\u{3000}\n", "\u{200b}"] {
		let created = assert_json(
			app.operator
				.post(
					&path,
					&json!({"title":content,"description":content}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
		assert_eq!(created["title"], content);
		assert_eq!(created["description"], content);
	}
}
