//! A Working Area's queue holds an execution only while it is awake (#1008,
//! ADR 0140 amendment of 2026-11-20): one that sleeps lets the next run, and
//! one that wakes re-enters at the tail.
use crate::native_database::{DatabaseFixture, database};
use crate::run_fixtures::{pending as encode_pending, run};
use aidash_application::tenancy::TenancyScope;
use aidash_server::{
	apps::execution::models::Run,
	domain::{NewTask, ReadyState, ResumeState, RunState, Thread, WaitingState},
	store::Store,
};
use chrono::{Duration, Utc};
use reinhardt::db::orm::Model;
use reinhardt::query::{Alias, PostgresQueryBuilder, Query, QueryStatementBuilder as _};
use rstest::rstest;
use serde_json::{Value, json};
use uuid::Uuid;

const DEPLOYMENT: &str = "aidash://area-queue";

/// A Thread of alice's with the Working Area of Agent `fixture` for her.
struct Queue {
	database: DatabaseFixture,
	store: Store,
	scope: TenancyScope,
	thread: Thread,
	area: Uuid,
	template: Run,
}

impl Queue {
	async fn new(database: DatabaseFixture, template: Run) -> Self {
		let store = Store::from_pool(
			database.connection.clone().into_postgres().unwrap(),
			aidash_domain::nodes::DeploymentId::parse(DEPLOYMENT).unwrap(),
		)
		.await
		.unwrap();
		let scope = crate::tenant_roots::tenant_scope(&store, "acme").await;
		let thread = store.create_thread(&scope, "alice", "Queue").await.unwrap();
		let area = Uuid::new_v4();
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("core_areas"))
				.columns(
					[
						"id",
						"tenant",
						"home_node",
						"agent_id",
						"owner",
						"thread_id",
					]
					.map(Alias::new),
				)
				.values_panic::<_, reinhardt::query::Value>([
					area.into(),
					"acme".into(),
					format!("{DEPLOYMENT}/{}", thread.node).into(),
					"fixture".into(),
					"alice".into(),
					thread.id.into(),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(store.pool.fixture_driver())
		.await
		.unwrap();
		Self {
			database,
			store,
			scope,
			thread,
			area,
			template,
		}
	}

	async fn task(&self, title: &str, dependencies: Vec<Uuid>) -> Uuid {
		self.store
			.create_task(
				&self.scope,
				self.thread.id,
				&NewTask {
					title: title.into(),
					description: "Area queue".into(),
					requirements: json!({}),
					dependencies,
					parent_id: None,
				},
				"alice",
				None,
			)
			.await
			.unwrap()
			.id
	}

	/// Admits an execution of `task` at the tail of the Area's queue, as an
	/// admission does.
	async fn admit(&self, task: Uuid) -> Uuid {
		let record = Run {
			id: Uuid::new_v4(),
			task_id: task,
			thread_id: self.thread.id,
			node: self.thread.node.to_string(),
			home_node: format!("{DEPLOYMENT}/{}", self.thread.node),
			updated_at: Utc::now(),
			..self.template.clone()
		};
		let mut db = self.database.lease.handle();
		let id = Run::objects()
			.create_with_conn(&mut db, &record)
			.await
			.unwrap()
			.id;
		sqlx::query(
			"INSERT INTO core_runs (run_id, area_id, sequence, initialized, tenant) SELECT $1, id, next_sequence, true, tenant FROM core_areas WHERE id = $2",
		)
		.bind(id)
		.bind(self.area)
		.execute(self.store.pool.fixture_driver())
		.await
		.unwrap();
		sqlx::query("UPDATE core_areas SET next_sequence = next_sequence + 1 WHERE id = $1")
			.bind(self.area)
			.execute(self.store.pool.fixture_driver())
			.await
			.unwrap();
		id
	}

	/// The execution a worker leases now. The recovery scan resumes after the
	/// last execution it looked at and starts over after an empty pass, so two
	/// passes see every execution.
	async fn lease(&self) -> Option<Uuid> {
		for _ in 0..2 {
			if let Some(run) = self.store.lease_run(Uuid::new_v4(), 30).await.unwrap() {
				return Some(run.id);
			}
		}
		None
	}

	/// Saves `state` and releases the lease, as a worker's step does.
	async fn save(&self, run: Uuid, phase: &str, state: Value) {
		sqlx::query(
			"UPDATE task_executions SET phase = $2, pending = $3, lease_owner = NULL, lease_until = NULL, revision = revision + 1 WHERE id = $1",
		)
		.bind(run)
		.bind(phase)
		.bind(state)
		.execute(self.store.pool.fixture_driver())
		.await
		.unwrap();
	}

	async fn sleep(&self, run: Uuid, wait: WaitingState) {
		self.save(
			run,
			"WAITING",
			encode_pending(RunState::Waiting(Box::new(wait)), Default::default()),
		)
		.await;
	}

	async fn finish(&self, run: Uuid) {
		self.save(
			run,
			"COMPLETED",
			encode_pending(
				RunState::Completed(aidash_server::domain::TerminalState {}),
				Default::default(),
			),
		)
		.await;
	}

	/// Where the execution stands in the queue: its position and whether it
	/// gave its place up.
	async fn place(&self, run: Uuid) -> (i64, bool) {
		sqlx::query_as("SELECT sequence, asleep FROM core_runs WHERE run_id = $1")
			.bind(run)
			.fetch_one(self.store.pool.fixture_driver())
			.await
			.unwrap()
	}

	async fn released(&self) -> Vec<Uuid> {
		sqlx::query_scalar(
			"SELECT run_id FROM run_activations WHERE reason = 'ordering_release' ORDER BY generation",
		)
		.fetch_all(self.store.pool.fixture_driver())
		.await
		.unwrap()
	}
}

fn timer(wake_at: chrono::DateTime<Utc>) -> WaitingState {
	WaitingState::Timer {
		wake_at,
		resume: ResumeState::Ready(ReadyState {}),
	}
}

/// Root Tasks X and Y in one Thread, for the same Agent and owner, share a
/// Working Area. X depends on Y and was admitted first: asleep on its
/// dependency, it lets Y run, and runs itself once Y has finished.
#[rstest]
#[tokio::test]
async fn a_run_waiting_on_a_task_of_its_thread_lets_that_task_run_first(
	#[future] database: DatabaseFixture,
	run: Run,
) {
	// Arrange
	let queue = Queue::new(database.await, run).await;
	let y_task = queue.task("Y", vec![]).await;
	let x_task = queue.task("X", vec![y_task]).await;
	let x = queue.admit(x_task).await;
	let y = queue.admit(y_task).await;
	assert_eq!(queue.lease().await, Some(x), "X heads the queue");
	// Act: X finds Y unfinished and sleeps on it, as the harness does.
	queue
		.sleep(
			x,
			WaitingState::Dependencies {
				wake_at: Utc::now() + Duration::hours(1),
				resume: ReadyState {},
			},
		)
		.await;
	// Assert: X gave its place up, and Y is told and leased.
	assert_eq!(queue.place(x).await, (1, true));
	assert_eq!(queue.released().await, vec![y]);
	assert_eq!(queue.lease().await, Some(y), "Y runs while X sleeps");
	// Y finishes; X wakes, re-enters at the tail and runs.
	queue.finish(y).await;
	sqlx::query("UPDATE tasks SET status = 'COMPLETED' WHERE id = $1")
		.bind(y_task)
		.execute(queue.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(queue.lease().await, Some(x), "X runs once Y is done");
	assert_eq!(queue.place(x).await, (3, false));
	queue.finish(x).await;
	assert_eq!(queue.lease().await, None);
}

/// An execution that wakes while another one of its Area runs re-enters the
/// queue at the tail and waits behind that head; it runs when the head ends.
#[rstest]
#[tokio::test]
async fn a_woken_execution_waits_behind_the_current_head(
	#[future] database: DatabaseFixture,
	run: Run,
) {
	// Arrange: A ran and sleeps; B, queued behind it, now runs.
	let queue = Queue::new(database.await, run).await;
	let a = queue.admit(queue.task("A", vec![]).await).await;
	let b = queue.admit(queue.task("B", vec![]).await).await;
	assert_eq!(queue.lease().await, Some(a));
	queue.sleep(a, timer(Utc::now() + Duration::hours(1))).await;
	assert_eq!(queue.lease().await, Some(b));
	// Act: A's wait is over while B runs.
	queue
		.sleep(a, timer(Utc::now() - Duration::seconds(1)))
		.await;
	let leased = queue.lease().await;
	// Assert: A is not leased; it waits at the tail, awake, behind B.
	assert_eq!(leased, None);
	assert_eq!(queue.place(a).await, (3, false));
	assert_eq!(queue.place(b).await, (2, false));
	// A new execution queues behind A.
	let c = queue.admit(queue.task("C", vec![]).await).await;
	assert_eq!(queue.place(c).await, (4, false));
	// B ends: A, the next awake execution, is told and leased before C.
	let before = queue.released().await.len();
	queue.finish(b).await;
	assert_eq!(queue.released().await[before..], [a]);
	assert_eq!(queue.lease().await, Some(a));
	assert_eq!(queue.lease().await, None, "C waits behind A");
}

/// A paused execution gives its place up like a sleeping one: the next one
/// of its Area runs, and when it is resumed it re-enters at the tail.
#[rstest]
#[tokio::test]
async fn a_paused_execution_lets_the_next_run_and_resumes_at_the_tail(
	#[future] database: DatabaseFixture,
	run: Run,
) {
	// Arrange
	let queue = Queue::new(database.await, run).await;
	let a = queue.admit(queue.task("A", vec![]).await).await;
	let b = queue.admit(queue.task("B", vec![]).await).await;
	// Act
	sqlx::query("UPDATE task_executions SET control = 'PAUSED' WHERE id = $1")
		.bind(a)
		.execute(queue.store.pool.fixture_driver())
		.await
		.unwrap();
	// Assert
	assert_eq!(queue.place(a).await, (1, true));
	assert_eq!(queue.lease().await, Some(b));
	sqlx::query("UPDATE task_executions SET control = 'ACTIVE' WHERE id = $1")
		.bind(a)
		.execute(queue.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(queue.lease().await, None, "A waits behind B");
	assert_eq!(queue.place(a).await, (3, false));
	queue.finish(b).await;
	assert_eq!(queue.lease().await, Some(a));
}

/// Ending the head tells the execution behind it that it may run; ending an
/// execution behind the head tells nobody.
#[rstest]
#[tokio::test]
async fn ending_an_execution_behind_the_head_releases_nothing(
	#[future] database: DatabaseFixture,
	run: Run,
) {
	let queue = Queue::new(database.await, run).await;
	let a = queue.admit(queue.task("A", vec![]).await).await;
	let b = queue.admit(queue.task("B", vec![]).await).await;
	let c = queue.admit(queue.task("C", vec![]).await).await;
	queue.finish(b).await;
	assert!(queue.released().await.is_empty());
	queue.finish(a).await;
	assert_eq!(queue.released().await, vec![c]);
}
