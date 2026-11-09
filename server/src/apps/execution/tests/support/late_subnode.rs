//! A Run's call that passed the tool's entrance just before its Subnode ended,
//! on a fixture that has no Subnode: the Run's own Workspace is made a Subnode,
//! the call is recorded as the model's choice, and the end is committed while
//! the call waits at the Subnode check of the write (#759).
//!
//! The entrance reads the Subnode without a lock, so a call that starts while
//! the end is uncommitted passes it. The write then waits for the end if, and
//! only if, it takes the shared Subnode lock first.
use aidash_server::{domain::Run, federation::Federation, harness::Harness};
use serde_json::{Value, json};
use uuid::Uuid;

/// Make `run`'s own Workspace an active Subnode whose only member is the Run's
/// Agent, with `allowance` as the tools its members may use. The Run is also the
/// creator, which the schema allows and nothing here reads.
#[allow(dead_code)]
pub async fn seed_subnode(f: &Federation, run: Uuid, allowance: &[String]) -> Uuid {
	let subnode = Uuid::new_v4();
	sqlx::query(
		"INSERT INTO subnodes (id, tenant, node, creator_run_id, creator_task_id, \
		  creator_agent_id, creator_agent_version, leader_agent_id, leader_agent_version, \
		  allowance, workspace_id, title, state, created_at, creation_key, creation_fingerprint) \
		 SELECT $1, tenant, node, id, task_id, agent_id, agent_version, agent_id, agent_version, \
		  $2, workspace_id, 'Late writes', 'active', now(), 'late', 'late' \
		 FROM runs WHERE id = $3",
	)
	.bind(subnode)
	.bind(json!({"tools":allowance,"secrets":[]}))
	.bind(run)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	sqlx::query(
		"INSERT INTO subnode_members (subnode_id, agent_id, tenant, agent_version, \
		  added_by_run_id, added_at) \
		 SELECT $1, agent_id, tenant, agent_version, id, now() FROM runs WHERE id = $2",
	)
	.bind(subnode)
	.bind(run)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	subnode
}

/// Record one model tool call on `run` for the worker to run next, as the
/// shared `state.rs` fixtures compose it. Only this file is included by the
/// targets that use it, so it does not borrow their `common` module.
#[allow(dead_code)]
pub async fn stage(f: &Federation, run: Uuid, name: &str, arguments: Value, call_id: &str) {
	let mut data = json!(aidash_server::domain::ToolCallState {
		response_epoch: 1,
		request_window: 120000,
		..Default::default()
	});
	let mut response = json!(aidash_server::provider::ModelResponse::default());
	response["tool_calls"] = json!([{"id":call_id,"name":name,"arguments":arguments}]);
	data["response"] = response;
	let state: aidash_server::domain::ToolCallState =
		serde_json::from_value(data).expect("a current ToolCall state");
	let pending = json!({
		"state_version": 1,
		"data": json!(aidash_server::domain::RunState::ToolCall(Box::new(state)))["data"],
		"recovery": {"retry": null},
	});
	sqlx::query("UPDATE runs SET phase = 'TOOL_CALL', pending = $1 WHERE id = $2")
		.bind(pending)
		.bind(run)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
}

/// The end of a Subnode, written but not committed: its row is locked and
/// marked dissolved in a transaction of the test's own.
#[allow(dead_code)]
pub struct EndFirst {
	tx: sqlx::Transaction<'static, sqlx::Postgres>,
	holder: i32,
	run: Uuid,
}

impl EndFirst {
	#[allow(dead_code)]
	pub async fn begin(f: &Federation, subnode: Uuid, run: Uuid) -> Self {
		let mut tx = f.store.pool.fixture_driver().begin().await.unwrap();
		let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
			.fetch_one(&mut *tx)
			.await
			.unwrap();
		sqlx::query("SELECT id FROM subnodes WHERE id = $1 FOR UPDATE")
			.bind(subnode)
			.fetch_one(&mut *tx)
			.await
			.unwrap();
		sqlx::query(
			"UPDATE subnodes SET state = 'dissolved', ended_at = now(), ended_reason = 'dissolved' WHERE id = $1",
		)
		.bind(subnode)
		.execute(&mut *tx)
		.await
		.unwrap();
		Self { tx, holder, run }
	}

	/// Wait, for at most 30 seconds, until a session waits on this transaction,
	/// and return what it is running. `None` when the call finished without
	/// waiting, which is what a write that is not fenced does: the test then
	/// goes on to show what that write left behind.
	#[allow(dead_code)]
	pub async fn until_a_call_waits(
		&self,
		f: &Federation,
		worker: &tokio::task::JoinHandle<bool>,
	) -> Option<Vec<String>> {
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
		loop {
			let waiting: Vec<String> = sqlx::query_scalar(
				"SELECT query FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))",
			)
			.bind(self.holder)
			.fetch_all(f.store.pool.fixture_driver())
			.await
			.unwrap();
			if !waiting.is_empty() {
				return Some(waiting);
			}
			if worker.is_finished() {
				return None;
			}
			assert!(
				tokio::time::Instant::now() < deadline,
				"the call neither waited for the end nor finished"
			);
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	}

	/// Cancel the Run in the same transaction, as the end does, and commit.
	#[allow(dead_code)]
	pub async fn commit(mut self) {
		sqlx::query("UPDATE runs SET control = 'CANCELLED' WHERE id = $1")
			.bind(self.run)
			.execute(&mut *self.tx)
			.await
			.unwrap();
		self.tx.commit().await.unwrap();
	}
}

/// The call waited at the shared lock the write takes on the Subnode, not at
/// some other lock.
#[allow(dead_code)]
pub fn assert_waited_at_the_subnode_check(waiting: Option<Vec<String>>) {
	let waiting = waiting.expect("the call finished without waiting: its write is not fenced");
	assert!(
		waiting
			.iter()
			.all(|query| query.contains("subnodes") && query.contains("FOR SHARE")),
		"the call must wait at the Subnode check: {waiting:?}"
	);
}

#[allow(dead_code)]
pub fn start_worker(f: &Federation) -> tokio::task::JoinHandle<bool> {
	let harness = Harness {
		federation: f.clone(),
	};
	tokio::spawn(async move { harness.worker_once().await.unwrap() })
}

/// Step the worker until the Run ends, and return how it ended.
#[allow(dead_code)]
pub async fn run_to_its_end(f: &Federation, run: Uuid) -> Run {
	let harness = Harness {
		federation: f.clone(),
	};
	for _ in 0..8 {
		let current = f
			.store
			.run(&aidash_application::tenancy::TEST_FIXTURE, run)
			.await
			.unwrap();
		if matches!(
			current.state.phase(),
			aidash_domain::RunPhase::Completed
				| aidash_domain::RunPhase::Failed
				| aidash_domain::RunPhase::Cancelled
		) {
			return current;
		}
		harness.worker_once().await.unwrap();
	}
	f.store
		.run(&aidash_application::tenancy::TEST_FIXTURE, run)
		.await
		.unwrap()
}

/// The Run ended as cancelled, not failed or paused.
#[allow(dead_code)]
pub fn assert_cancelled(run: &Run) {
	assert_eq!(
		run.state.phase(),
		aidash_domain::RunPhase::Cancelled,
		"the Run must end as cancelled, not failed or paused: control {:?}, error {:?}",
		run.control,
		run.error
	);
	assert_eq!(run.control, aidash_domain::RunControl::Cancelled);
}
