//! Runs on the same Node Scratch Space can hold worker leases concurrently.
use crate::native_database::{DatabaseFixture, database};
use crate::run_fixtures::run;
use aidash_application::tenancy::TenancyScope;
use aidash_server::{
	apps::execution::models::Run,
	domain::{NewTask, Thread},
	store::Store,
};
use chrono::Utc;
use reinhardt::db::orm::Model;
use reinhardt::query::{Alias, PostgresQueryBuilder, Query, QueryStatementBuilder as _};
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

const DEPLOYMENT: &str = "aidash://shared-scratch";

/// A Thread whose two Runs share the same Node Scratch Space.
struct ScratchRuns {
	database: DatabaseFixture,
	store: Store,
	scope: TenancyScope,
	thread: Thread,
	area: Uuid,
	template: Run,
}

impl ScratchRuns {
	async fn new(database: DatabaseFixture, template: Run) -> Self {
		let store = Store::from_pool(
			database.connection.clone().into_postgres().unwrap(),
			aidash_domain::nodes::DeploymentId::parse(DEPLOYMENT).unwrap(),
		)
		.await
		.unwrap();
		let scope = crate::tenant_roots::tenant_scope(&store, "acme").await;
		let thread = store
			.create_thread(&scope, "alice", "Shared Scratch")
			.await
			.unwrap();
		let area = Uuid::new_v4();
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("core_execution_scopes"))
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
					description: "Shared Scratch".into(),
					goal: None,
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

	/// Create a Run and its retained capability binding.
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
			"INSERT INTO core_runs (run_id, scope_subject, initialized, tenant) SELECT $1, owner, true, tenant FROM core_execution_scopes WHERE id = $2",
		)
		.bind(id)
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
}

#[rstest]
#[tokio::test]
async fn two_runs_sharing_the_node_space_can_hold_leases_at_once(
	#[future] database: DatabaseFixture,
	run: Run,
) {
	let fixture = ScratchRuns::new(database.await, run).await;
	let first = fixture.admit(fixture.task("First", vec![]).await).await;
	let second = fixture.admit(fixture.task("Second", vec![]).await).await;
	let bound_to_default: bool = sqlx::query_scalar(
		"SELECT s.is_default FROM threads t JOIN scratch_spaces s ON s.id = t.scratch_space_id
		 WHERE t.id = $1 AND t.tenant = s.tenant AND t.node = s.node",
	)
	.bind(fixture.thread.id)
	.fetch_one(fixture.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(bound_to_default);

	let leased = [
		fixture.lease().await.unwrap(),
		fixture.lease().await.unwrap(),
	];
	assert!(leased.contains(&first));
	assert!(leased.contains(&second));
	assert_eq!(fixture.lease().await, None);
}
