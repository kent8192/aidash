use crate::native_database::{DatabaseFixture, database};
use aidash_server::apps::workspaces::models::{Task, TaskDependency};
use aidash_server::{Error, domain::NewTask, store::Store};
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn task() -> NewTask {
	NewTask {
		title: "Task".into(),
		description: "Graph test".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	}
}

#[rstest]
#[tokio::test]
async fn combined_parent_dependency_cycle_is_rejected_atomically(
	#[future] database: DatabaseFixture,
	mut task: NewTask,
) {
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://graph".into(),
	)
	.await
	.unwrap();
	let workspace = store
		.create_workspace("Graph", "Cycle validation")
		.await
		.unwrap();
	let ancestor = store
		.create_task(workspace.id, &task, "operator", None)
		.await
		.unwrap();
	task.dependencies = vec![ancestor.id];
	let dependent = store
		.create_task(workspace.id, &task, "operator", None)
		.await
		.unwrap();
	// ancestor -> child -> dependent -> ancestor is a cycle even though the
	// child's immediate dependency is not one of its ancestors.
	task.parent_id = Some(ancestor.id);
	task.dependencies = vec![dependent.id];
	let rejected = store
		.create_task(workspace.id, &task, "operator", Some("cycle"))
		.await;
	// The latest development schema rejects the combined graph through its
	// deferred constraint trigger. Preserve its failure class and rollback.
	let Err(Error::Database(sqlx::Error::Database(error))) = rejected else {
		panic!("expected the preserved dependency constraint: {rejected:?}");
	};
	assert_eq!(error.code().as_deref(), Some("23514"));
	assert_eq!(error.constraint(), Some("tasks_dependency_cycle"));
	assert_eq!(store.tasks(Some(workspace.id)).await.unwrap().len(), 2);
}

#[rstest]
#[tokio::test]
async fn dependency_projection_is_atomic_and_idempotent(
	#[future] database: DatabaseFixture,
	mut task: NewTask,
) {
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://graph".into(),
	)
	.await
	.unwrap();
	let workspace = store
		.create_workspace("Graph", "Reference projection")
		.await
		.unwrap();
	let dependency = store
		.create_task(workspace.id, &task, "operator", None)
		.await
		.unwrap();
	task.dependencies = vec![dependency.id, dependency.id];
	let saved = store
		.create_task(workspace.id, &task, "operator", Some("projection"))
		.await
		.unwrap();
	let repeated = store
		.create_task(workspace.id, &task, "operator", Some("projection"))
		.await
		.unwrap();
	assert_eq!(saved.id, repeated.id);
	let links = TaskDependency::objects()
		.filter(TaskDependency::field_task_key().eq(saved.id))
		.all_with_db(&mut database.lease.handle())
		.await
		.unwrap();
	assert_eq!(links.len(), 1);
	assert_eq!(links[0].dependency_key, dependency.id);
	assert_eq!(links[0].workspace_key, workspace.id);

	let other = store
		.create_workspace("Other", "Keep dependencies local")
		.await
		.unwrap();
	let rejected = store.create_task(other.id, &task, "operator", None).await;
	assert!(matches!(rejected, Err(Error::Invalid(_))), "{rejected:?}");
	assert!(store.tasks(Some(other.id)).await.unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn dependency_relations_prevent_target_deletion_and_workspace_changes(
	#[future] database: DatabaseFixture,
	mut task: NewTask,
) {
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://graph".into(),
	)
	.await
	.unwrap();
	let workspace = store
		.create_workspace("Graph", "Reverse relations")
		.await
		.unwrap();
	let other = store
		.create_workspace("Other", "Keep references local")
		.await
		.unwrap();
	let target = store
		.create_task(workspace.id, &task, "operator", None)
		.await
		.unwrap();
	task.dependencies = vec![target.id];
	let source = store
		.create_task(workspace.id, &task, "operator", None)
		.await
		.unwrap();
	let mut db = database.lease.handle();
	assert!(
		Task::objects()
			.delete_with_conn(&mut db, target.id)
			.await
			.is_err()
	);
	for id in [source.id, target.id] {
		assert!(
			Task::objects()
				.filter(Task::field_id().eq(id))
				.update_fields_with_conn(&mut db, [(Task::field_workspace_id(), other.id)])
				.await
				.is_err()
		);
		let unchanged = Task::objects().get(id).get_with_db(&mut db).await.unwrap();
		assert_eq!(unchanged.workspace_id, workspace.id);
	}
	Task::objects()
		.delete_with_conn(&mut db, source.id)
		.await
		.unwrap();
	let edges = TaskDependency::objects()
		.filter(TaskDependency::field_task_key().eq(source.id))
		.all_with_db(&mut db)
		.await
		.unwrap();
	assert!(
		edges.is_empty(),
		"deleting the source must cascade its edge projection"
	);
	Task::objects()
		.delete_with_conn(&mut db, target.id)
		.await
		.unwrap();
}

#[rstest]
#[tokio::test]
async fn concurrent_dependency_creation_and_target_deletion_never_leave_dangling_edges(
	#[future] database: DatabaseFixture,
	mut task: NewTask,
) {
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://graph".into(),
	)
	.await
	.unwrap();
	let workspace = store
		.create_workspace("Graph", "Concurrent references")
		.await
		.unwrap();
	let target = store
		.create_task(workspace.id, &task, "operator", None)
		.await
		.unwrap();
	task.dependencies = vec![target.id];
	let mut db = database.lease.handle();
	let tasks = Task::objects();
	let (created, deleted) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
		tokio::join!(
			store.create_task(workspace.id, &task, "operator", None),
			tasks.delete_with_conn(&mut db, target.id)
		)
	})
	.await
	.expect("competing reference writes must finish");
	assert_ne!(
		created.is_ok(),
		deleted.is_ok(),
		"both competing writes must not commit"
	);
	let tasks = Task::objects()
		.filter(Task::field_workspace_id().eq(workspace.id))
		.all_with_db(&mut db)
		.await
		.unwrap();
	let edges = TaskDependency::objects()
		.filter(TaskDependency::field_workspace_key().eq(workspace.id))
		.all_with_db(&mut db)
		.await
		.unwrap();
	if let Ok(source) = created {
		assert_eq!(tasks.len(), 2);
		assert_eq!(edges.len(), 1);
		assert_eq!(edges[0].task_key, source.id);
		assert_eq!(edges[0].dependency_key, target.id);
	} else {
		assert!(
			tasks.is_empty(),
			"failed creation must not leave a partial source"
		);
		assert!(edges.is_empty());
	}
}
