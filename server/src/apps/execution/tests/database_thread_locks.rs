//! The Thread row locks of admission, deletion and writes (#1008).
use crate::native_database::{DatabaseFixture, database};
use aidash_server::{apps::workspaces::models::Thread, domain::NewTask, store::Store};
use rstest::rstest;
use serde_json::json;
use std::time::Duration;

async fn store(database: &DatabaseFixture) -> (Store, aidash_application::tenancy::TenancyScope) {
	let store = Store::from_pool(
		database.connection.clone().into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://thread-locks").unwrap(),
	)
	.await
	.unwrap();
	let scope = crate::tenant_roots::tenant_scope(&store, "acme").await;
	(store, scope)
}

/// Waits until some statement waits for a row lock.
async fn blocked(store: &Store) {
	tokio::time::timeout(Duration::from_secs(10), async {
		loop {
			let waiting: i64 = sqlx::query_scalar(
				"SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND datname = current_database()",
			)
			.fetch_one(store.pool.fixture_driver())
			.await
			.unwrap();
			if waiting > 0 {
				return;
			}
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	})
	.await
	.expect("a writer waiting for the deletion's lock");
}

/// An admission locks its Thread `FOR NO KEY UPDATE`: an event written in the
/// Thread meanwhile does not wait for it, but a deletion still does.
#[rstest]
#[tokio::test]
async fn an_admission_holds_back_a_deletion_but_not_an_event_of_its_thread(
	#[future] database: DatabaseFixture,
) {
	// Arrange: an admission in progress holds the Thread.
	let database = database.await;
	let (store, scope) = store(&database).await;
	let thread = store.create_thread(&scope, "alice", "Locks").await.unwrap();
	let mut admission = database.begin_in(&scope).await;
	Thread::lock_existing(&mut *admission, thread.id)
		.await
		.unwrap();
	// Act: an event in the Thread, and a deletion, each bounded by a lock timeout.
	let mut writer = database.begin_in(&scope).await;
	aidash_server::database::native::query("SET LOCAL lock_timeout = '2s'")
		.execute(&mut *writer)
		.await
		.unwrap();
	let event = store
		.event(&mut writer, Some(thread.id), "thread.updated", json!({}))
		.await;
	let mut deletion = database.begin_in(&scope).await;
	aidash_server::database::native::query("SET LOCAL lock_timeout = '200ms'")
		.execute(&mut *deletion)
		.await
		.unwrap();
	let deleting = Thread::lock_for_deletion(&mut *deletion, thread.id).await;
	// Assert
	assert!(event.is_ok(), "{event:?}");
	writer.commit().await.unwrap();
	let error = deleting.expect_err("a deletion waits for the admission");
	assert!(error.to_string().contains("lock"), "{error}");
	deletion.rollback().await.unwrap();
	admission.rollback().await.unwrap();
}

/// A Task, a message or a title change that waits for a deletion of its
/// Thread is refused when the deletion commits: nothing commits into a
/// deleted Thread.
#[rstest]
#[case::task("task")]
#[case::message("message")]
#[case::title("title")]
#[tokio::test]
async fn a_write_waiting_for_a_deletion_is_refused_when_it_commits(
	#[future] database: DatabaseFixture,
	#[case] write: &'static str,
) {
	// Arrange: a deletion has locked and marked the Thread, uncommitted.
	let database = database.await;
	let (store, scope) = store(&database).await;
	let thread = store.create_thread(&scope, "alice", "Locks").await.unwrap();
	let before = contents(&store, thread.id).await;
	let mut deletion = database.begin_in(&scope).await;
	Thread::lock_for_deletion(&mut *deletion, thread.id)
		.await
		.unwrap();
	aidash_server::database::native::query(&format!(
		"UPDATE threads SET deleted_at = CURRENT_TIMESTAMP WHERE id = '{}'",
		thread.id
	))
	.execute(&mut *deletion)
	.await
	.unwrap();
	// Act: the write starts, waits for the deletion, which then commits.
	let writing = {
		let (store, scope, id, revision) =
			(store.clone(), scope.clone(), thread.id, thread.revision);
		tokio::spawn(async move {
			match write {
				"task" => store
					.create_task(
						&scope,
						id,
						&NewTask {
							task_model: None,
							title: "Late".into(),
							description: "Must not land".into(),
							requirements: json!({}),
							dependencies: vec![],
							parent_id: None,
						},
						"alice",
						None,
					)
					.await
					.map(|_| ()),
				"message" => store.message(&scope, id, "alice", "Too late", None).await,
				_ => store
					.update_title(&scope, id, revision, "Renamed")
					.await
					.map(|_| ()),
			}
		})
	};
	blocked(&store).await;
	deletion.commit().await.unwrap();
	let written = writing.await.unwrap();
	// Assert
	assert!(written.is_err(), "{write} committed into a deleted Thread");
	assert_eq!(contents(&store, thread.id).await, before);
}

/// The Thread's Tasks, messages and title.
async fn contents(store: &Store, thread: uuid::Uuid) -> (i64, i64, String) {
	sqlx::query_as(
		"SELECT (SELECT count(*) FROM tasks WHERE thread_id = $1), (SELECT count(*) FROM messages WHERE thread_id = $1), (SELECT title FROM threads WHERE id = $1)",
	)
	.bind(thread)
	.fetch_one(store.pool.fixture_driver())
	.await
	.unwrap()
}
