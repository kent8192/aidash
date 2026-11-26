//! Duty runs use the Node Scratch Space even while another Run is waiting.
use super::*;
use aidash_server::apps::workspaces::services::duties::pass;
use aidash_server::domain::{ReadyState, RecoveryState, ResumeState, RunState, WaitingState};
use chrono::{Duration as Span, Utc};

/// The execution of `task`.
async fn execution(c: &CoreFixture, task: Uuid) -> Uuid {
	sqlx::query_scalar("SELECT id FROM task_executions WHERE task_id = $1")
		.bind(task)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

/// A Duty's next Run can be leased while the preceding Run waits for a Task.
#[rstest::rstest]
#[tokio::test]
async fn a_duty_task_asleep_on_task_wait_does_not_hold_back_the_next(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: a Duty's tick started a Task, whose execution sleeps in
	// `task_wait` (as the harness leaves it).
	let c = capability_fixture.await;
	let origin =
		c.f.store
			.task(&TEST_FIXTURE, c.task)
			.await
			.unwrap()
			.thread_id;
	let (status, duty) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/threads/{origin}/duties"),
		json!({
			"title": "CI watcher",
			"mission": "Report what CI says.",
			"agent": {"id": "research", "version": "1.1.0"},
			"trigger": {"kind": "interval", "every_seconds": 3600},
		}),
	)
	.await;
	assert_eq!(status, 200, "{duty}");
	let id: Uuid = serde_json::from_value(duty["id"].clone()).unwrap();
	let due = Utc::now() - Span::seconds(90);
	sqlx::query(
		"UPDATE duties SET anchor = $1 - interval '1 day', next_scheduled_at = $1, next_fire_at = $1 WHERE id = $2",
	)
	.bind(due)
	.bind(id)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let first = pass(&c.f, Utc::now()).await.unwrap();
	assert_eq!(first.started.len(), 1, "{first:?}");
	let asleep = execution(&c, first.started[0]).await;
	let state = json!(RunState::Waiting(Box::new(WaitingState::TaskWait {
		wake_at: Utc::now() + Span::days(7),
		cursor: 0,
		resume: ResumeState::Ready(ReadyState {}),
	})));
	sqlx::query("UPDATE task_executions SET phase = 'WAITING', pending = $2 WHERE id = $1")
		.bind(asleep)
		.bind(
			json!({"state_version": 1, "data": state["data"], "recovery": RecoveryState::default()}),
		)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	// Act: the Duty is run again while its Task sleeps.
	let (status, second) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/duties/{id}/run-now"),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{second}");
	let next = execution(
		&c,
		serde_json::from_value(second["task_id"].clone()).unwrap(),
	)
	.await;
	// Assert: both Runs use this Node's default Scratch Space.
	let spaces: Vec<(Uuid, bool)> = sqlx::query_as(
		"SELECT s.id, s.is_default FROM task_executions r
		 JOIN threads t ON t.id = r.thread_id AND t.tenant = r.tenant
		 JOIN scratch_spaces s ON s.id = t.scratch_space_id AND s.tenant = t.tenant
		 WHERE r.id IN ($1, $2)",
	)
	.bind(asleep)
	.bind(next)
	.fetch_all(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(spaces.len(), 2, "{spaces:?}");
	assert_eq!(spaces[0], spaces[1]);
	assert!(spaces[0].1);
	let mut leased = vec![];
	for _ in 0..8 {
		let Some(run) = c.f.store.lease_run(Uuid::new_v4(), 30).await.unwrap() else {
			break;
		};
		leased.push(run.id);
	}
	assert!(leased.contains(&next), "{leased:?}");
	assert!(!leased.contains(&asleep), "{leased:?}");
	c.close().await;
}
