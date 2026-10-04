//! Task graph constraints and their HTTP transaction boundary.
use crate::endpoint::{EndpointFixture, assert_json, endpoint, workspace};
use aidash_server::apps::execution::models::Event;
use aidash_server::apps::workspaces::models::{Task, TaskDependency};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::{Value, json};
use uuid::Uuid;

async fn create(app: &EndpointFixture, path: &str, input: &Value) -> Value {
	assert_json(app.operator.post(path, input, "json").await.unwrap(), 200)
}

#[rstest]
#[tokio::test]
async fn missing_and_foreign_task_edges_cannot_create_partial_records(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let home = workspace(&app.operator, "Home").await;
	let other = workspace(&app.operator, "Other").await;
	let home_path = format!("/api/workspaces/{}/tasks", home["id"].as_str().unwrap());
	let other_path = format!("/api/workspaces/{}/tasks", other["id"].as_str().unwrap());
	let foreign = create(
		&app,
		&other_path,
		&json!({"title":"Foreign","description":"Other workspace"}),
	)
	.await;
	for target in [json!(Uuid::new_v4()), foreign["id"].clone()] {
		for field in ["parent_id", "dependencies"] {
			let mut input = json!({"title":"Rejected","description":"Invalid edge"});
			input[field] = if field == "dependencies" {
				json!([target])
			} else {
				target.clone()
			};
			assert_json(
				app.operator.post(&home_path, &input, "json").await.unwrap(),
				400,
			);
		}
	}
	let home_id = serde_json::from_value::<Uuid>(home["id"].clone()).unwrap();
	assert!(
		Task::objects()
			.filter(Task::field_workspace_id().eq(home_id))
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	assert!(
		TaskDependency::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	assert!(
		Event::objects()
			.filter(Event::field_workspace_id().eq(Some(home_id)))
			.filter(Event::field_kind().eq("task.created".to_owned()))
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
}

#[rstest]
#[tokio::test]
async fn diamond_graph_reachability_is_bounded_and_combined_cycles_are_rejected(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let home = workspace(&app.operator, "Diamond graph").await;
	let path = format!("/api/workspaces/{}/tasks", home["id"].as_str().unwrap());
	let mut levels: Vec<Vec<Value>> = vec![];
	for depth in 0..20 {
		let dependencies = levels.last().cloned().unwrap_or_default();
		let mut level = vec![];
		for side in ["left", "right"] {
			let task = create(&app, &path, &json!({"title":format!("{depth}-{side}"),"description":"Shared ancestors","dependencies":dependencies})).await;
			level.push(task["id"].clone());
		}
		levels.push(level);
	}
	let cycle = json!({"title":"Cycle","description":"Reject before projection","parent_id":levels[0][0],"dependencies":[levels.last().unwrap()[0]]});
	let response = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		app.operator.post(&path, &cycle, "json"),
	)
	.await
	.expect("diamond traversal must not enumerate every path")
	.unwrap();
	// Preserve the development baseline's constraint-failure envelope.
	assert_eq!(
		assert_json(response, 500)["error"],
		"operation failed; see server logs"
	);
	assert_eq!(
		Task::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.len(),
		40
	);
	assert_eq!(
		TaskDependency::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.len(),
		76
	);
}

#[rstest]
#[tokio::test]
async fn concurrent_parent_and_dependency_changes_cannot_commit_a_cycle(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let home = workspace(&app.operator, "Concurrent graph").await;
	let path = format!("/api/workspaces/{}/tasks", home["id"].as_str().unwrap());
	let left = create(&app, &path, &json!({"title":"Left","description":"Root"})).await;
	let right = create(&app, &path, &json!({"title":"Right","description":"Root"})).await;
	let first = json!({"title":"First child","description":"Left to right","parent_id":left["id"],"dependencies":[right["id"]]});
	let second = json!({"title":"Second child","description":"Right to left","parent_id":right["id"],"dependencies":[left["id"]]});
	let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
		tokio::join!(
			app.operator.post(&path, &first, "json"),
			app.operator.post(&path, &second, "json")
		)
	})
	.await
	.expect("competing graph changes must terminate");
	let (first, second) = (first.unwrap(), second.unwrap());
	let mut statuses = [first.status_code(), second.status_code()];
	statuses.sort();
	assert_eq!(statuses, [200, 500], "{}; {}", first.text(), second.text());
	assert_eq!(
		Task::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.len(),
		3
	);
	assert_eq!(
		TaskDependency::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.len(),
		1
	);
	assert_eq!(
		Event::objects()
			.filter(Event::field_kind().eq("task.created".to_owned()))
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.len(),
		3
	);
}
