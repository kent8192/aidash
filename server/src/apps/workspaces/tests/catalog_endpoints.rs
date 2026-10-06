use crate::endpoint::{EndpointFixture, assert_json, endpoint, workspace};
use aidash_server::apps::workspaces::models::Task;
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn task_pages_preserve_timestamp_ties_and_public_fields(#[future] endpoint: EndpointFixture) {
	let app = endpoint.await;
	let workspace = workspace(&app.operator, "Task discovery").await;
	let created = assert_json(
		app.operator
			.post(
				&format!(
					"/api/workspaces/{}/tasks",
					workspace["id"].as_str().unwrap()
				),
				&json!({"title":"First task", "description":"Discovery boundary"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let id: Uuid = serde_json::from_value(created["id"].clone()).unwrap();
	let db = &mut app.database.lease.handle();
	let original = Task::objects()
		.filter(Task::field_id().eq(id))
		.get_with_db(db)
		.await
		.unwrap();
	// Keep every creation timestamp identical so UUID ordering decides the
	// page boundary. Native fixtures avoid hundreds of unrelated HTTP writes.
	let copies: Vec<_> = (1..=500)
		.map(|index| Task {
			id: Uuid::from_u128(index),
			title: format!("Task {index}"),
			..original.clone()
		})
		.collect();
	let mut expected: Vec<_> = copies.iter().map(|task| task.id).collect();
	expected.push(original.id);
	expected.sort_unstable_by(|left, right| right.cmp(left));
	Task::objects()
		.bulk_create_with_conn(db, copies, Some(100), false, false)
		.await
		.unwrap();

	let first = assert_json(app.operator.get("/api/tasks").await.unwrap(), 200);
	let second = assert_json(
		app.operator.get("/api/tasks?offset=500").await.unwrap(),
		200,
	);
	assert_eq!(first["next_offset"], 500);
	assert_eq!(second["next_offset"], json!(null));
	let first_tasks = first["tasks"].as_array().unwrap();
	let second_tasks = second["tasks"].as_array().unwrap();
	assert_eq!(first_tasks.len(), 500);
	assert_eq!(second_tasks.len(), 1);
	let actual: Vec<Uuid> = first_tasks
		.iter()
		.chain(second_tasks)
		.map(|task| {
			assert_eq!(task["workspace_id"], workspace["id"]);
			assert_eq!(task["status"], "OPEN");
			assert_eq!(task["requirements"], json!({}));
			assert_eq!(task["created_at"], created["created_at"]);
			assert!(task.get("creation_key").is_none());
			assert!(task.get("completion_key").is_none());
			serde_json::from_value(task["id"].clone()).unwrap()
		})
		.collect();
	assert_eq!(actual, expected);
	let exhausted = assert_json(
		app.operator.get("/api/tasks?offset=501").await.unwrap(),
		200,
	);
	assert_eq!(exhausted["tasks"], json!([]));
	assert_eq!(exhausted["next_offset"], json!(null));
}
