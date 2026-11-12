//! A person declines the tool call an Authority Pause stopped (#763): which pauses record the
//! call, and what declining writes.
use super::Store;
use super::targeted_leases::run_fixtures::{self, run};
use crate::Error;
use crate::apps::execution::models::{
	Run as Record,
	states::{RunControl, RunPhase},
};
use crate::apps::execution::tests::native_database::{DatabaseFixture, database};
use crate::domain::{RecoveryState, RunState, ToolCallState};
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_domain::provider::{ModelResponse, ToolCall};
use aidash_domain::tool::{AuthorityRefusal, RefusalCheck, RefusalKind, RefusedCall};
use chrono::{Duration, Utc};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::{Value, json};
use uuid::Uuid;

const EPOCH: i64 = 5;

struct World {
	store: Store,
	id: Uuid,
	worker: Uuid,
	key: String,
	database: DatabaseFixture,
}

/// A Run leased by `worker` and executing the only call of a model response.
async fn world(database: DatabaseFixture, run: Record) -> World {
	let store = Store::from_pool(
		database.connection.clone().into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://declined-calls-test").unwrap(),
	)
	.await
	.unwrap();
	database.tenant("acme").await;
	let state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			tool_calls: vec![ToolCall {
				id: "call-0".into(),
				name: "task_delegate".into(),
				arguments: json!({"task_id": Uuid::nil()}),
			}],
			..Default::default()
		},
		response_epoch: EPOCH,
		..Default::default()
	}));
	let worker = Uuid::new_v4();
	let record = Record {
		phase: serde_json::from_value::<RunPhase>(json!(state.phase())).unwrap(),
		control: RunControl::Active,
		pending: run_fixtures::pending(state, RecoveryState::default()).into(),
		lease_owner: Some(worker),
		lease_until: Some(Utc::now() + Duration::minutes(1)),
		..run
	};
	let mut db = database.lease.handle();
	Record::objects()
		.create_with_conn(&mut db, &record)
		.await
		.unwrap();
	World {
		key: format!("{}:{EPOCH}:0", record.id),
		id: record.id,
		store,
		worker,
		database,
	}
}

impl World {
	fn refusal(&self, check: RefusalCheck, call: bool) -> AuthorityRefusal {
		AuthorityRefusal {
			tool: "task_delegate".into(),
			check,
			action: Some("task.delegate".into()),
			subject: None,
			call: call.then(|| RefusedCall {
				key: self.key.clone(),
				index: 0,
				replay_safe: true,
				tool_call: None,
			}),
		}
	}
	async fn run(&self) -> crate::domain::Run {
		self.store.run(&TEST_FIXTURE, self.id).await.unwrap()
	}
	/// Journal the call as `status` says: nothing, refused, started or completed.
	async fn journal(&self, status: &str) {
		if status == "none" {
			return;
		}
		let run = self.run().await;
		let args = json!({"task_id": Uuid::nil()});
		self.store
			.invocation_start(
				&TEST_FIXTURE,
				&run,
				self.worker,
				&self.key,
				"task_delegate",
				&args,
				true,
			)
			.await
			.unwrap();
		match status {
			"refused" => self
				.store
				.invocation_refused(
					&TEST_FIXTURE,
					&run,
					self.worker,
					&self.key,
					&self.refusal(RefusalCheck::Policy, true),
				)
				.await
				.unwrap(),
			"completed" => {
				self.store
					.invocation_finish(
						&TEST_FIXTURE,
						&run,
						self.worker,
						&self.key,
						&json!({"ok": true}),
					)
					.await
					.unwrap();
			}
			_ => {}
		}
	}
	async fn pause(&self, refusal: &AuthorityRefusal) {
		let run = self.run().await;
		self.store
			.pause_for_execution(
				&run,
				self.worker,
				"task_delegate: task.delegate was denied",
				"run.authorization_blocked",
				Some(refusal),
			)
			.await
			.unwrap();
	}
	async fn recovery(&self) -> Value {
		let mut db = self.database.lease.handle();
		let record = Record::objects()
			.filter(Record::field_id().eq(self.id))
			.get_with_db(&mut db)
			.await
			.unwrap();
		serde_json::to_value(&record.pending).unwrap()["recovery"].clone()
	}
	async fn row(&self) -> Option<(String, Option<Value>)> {
		sqlx::query_as::<_, (String, Option<Value>)>(
			"SELECT status, result FROM invocations WHERE idempotency_key = $1",
		)
		.bind(&self.key)
		.fetch_optional(self.store.pool.fixture_driver())
		.await
		.unwrap()
	}
	async fn events(&self) -> Vec<String> {
		sqlx::query_scalar::<_, String>("SELECT kind FROM events WHERE data->>'run_id' = $1")
			.bind(self.id.to_string())
			.fetch_all(self.store.pool.fixture_driver())
			.await
			.unwrap()
	}
	async fn decline(&self, call: &str, by: &str) -> crate::Result<()> {
		let mut tx = self.store.pool.begin(&TEST_FIXTURE).await.unwrap();
		let declined = self
			.store
			.decline_call_in(&mut tx, self.id, call, by, false)
			.await?;
		assert_eq!(declined.call.tool, "task_delegate");
		tx.commit().await.unwrap();
		Ok(())
	}
}

#[rstest]
#[case::no_row("none", RefusalCheck::Policy, true, true)]
#[case::refused_row("refused", RefusalCheck::Policy, true, true)]
#[case::started_row("started", RefusalCheck::Policy, true, false)]
#[case::completed_row("completed", RefusalCheck::Policy, true, false)]
#[case::run_authority("none", RefusalCheck::RunAuthority, true, false)]
#[case::tenant_deletion("none", RefusalCheck::TenantDeletion, true, false)]
#[case::remote_ask("none", RefusalCheck::RemoteAsk, true, false)]
#[case::no_call_named("none", RefusalCheck::Policy, false, false)]
#[tokio::test]
async fn a_pause_records_its_call_only_when_the_call_certainly_did_not_run(
	#[future] database: DatabaseFixture,
	run: Record,
	#[case] row: &str,
	#[case] check: RefusalCheck,
	#[case] names_call: bool,
	#[case] recorded: bool,
) {
	let w = world(database.await, run).await;
	w.journal(row).await;
	w.pause(&w.refusal(check, names_call)).await;
	let recovery = w.recovery().await;
	assert_eq!(
		recovery.get("paused_call").is_some(),
		recorded,
		"{recovery}"
	);
	if recorded {
		assert_eq!(recovery["paused_call"]["key"], w.key);
		assert_eq!(recovery["paused_call"]["tool"], "task_delegate");
		assert_eq!(recovery["paused_call"]["kind"], "policy");
	}
	// The event never carries the call's key.
	assert!(
		w.events()
			.await
			.contains(&"run.authorization_blocked".to_owned())
	);
}

#[rstest]
#[tokio::test]
async fn a_plain_pause_by_a_person_is_not_declinable(
	#[future] database: DatabaseFixture,
	run: Record,
) {
	let w = world(database.await, run).await;
	// A person paused the Run first; the worker's later refusal does not record a call.
	sqlx::query("UPDATE runs SET control = 'PAUSED' WHERE id = $1")
		.bind(w.id)
		.execute(w.store.pool.fixture_driver())
		.await
		.unwrap();
	w.pause(&w.refusal(RefusalCheck::Policy, true)).await;
	assert!(w.recovery().await.get("paused_call").is_none());
	assert!(matches!(
		w.decline(&w.key, "alice").await,
		Err(Error::Conflict(_))
	));
}

#[rstest]
#[case::no_row("none")]
#[case::refused_row("refused")]
#[tokio::test]
async fn declining_completes_the_call_with_the_fixed_sentence_and_resumes_the_run(
	#[future] database: DatabaseFixture,
	run: Record,
	#[case] row: &str,
) {
	let w = world(database.await, run).await;
	w.journal(row).await;
	w.pause(&w.refusal(RefusalCheck::Policy, true)).await;
	w.decline(&w.key, "alice").await.unwrap();

	let sentence = RefusalKind::Policy.declined_sentence();
	let (status, result) = w.row().await.expect("a declined call has a row");
	assert_eq!(status, "COMPLETED");
	assert_eq!(result, Some(json!({"error": sentence})));
	// Nothing started, so there is no start event; the call shows as answered.
	let events = w.events().await;
	assert!(!events.contains(&"tool.started".to_owned()) || row == "refused");
	assert!(events.contains(&"tool.completed".to_owned()));
	assert!(events.contains(&"run.call_declined".to_owned()));
	// The Run went on; the pause is forgotten and the decision is kept for the worker.
	let run = w.run().await;
	assert_eq!(run.control, crate::domain::RunControl::Active);
	let recovery = w.recovery().await;
	assert!(recovery.get("paused_call").is_none(), "{recovery}");
	assert_eq!(
		recovery["declined_calls"],
		json!([{"key": w.key, "kind": "policy"}])
	);
	let event: Value = sqlx::query_scalar(
		"SELECT data FROM events WHERE kind = 'run.call_declined' AND data->>'run_id' = $1",
	)
	.bind(w.id.to_string())
	.fetch_one(w.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(event["by"], "alice");
	assert_eq!(event["tool"], "task_delegate");
	assert_eq!(event["kind"], "policy");
	// Never the action.
	assert!(!event.to_string().contains("task.delegate"), "{event}");
}

#[rstest]
#[tokio::test]
async fn decline_records_the_call_from_the_filtered_response(
	#[future] database: DatabaseFixture,
	run: Record,
) {
	let w = world(database.await, run).await;
	// The worker's cursor indexes the catch-up list, after the first call is filtered out.
	let calls = json!([
		{"id":"discarded", "name":"unadvertised", "arguments":{"wrong":true}},
		{"id":"kept", "name":"task_delegate", "arguments":{"task_id":"correct"}}
	]);
	sqlx::query(
		"UPDATE runs SET pending = jsonb_set(pending, '{data,response,tool_calls}', $1) WHERE id = $2",
	)
	.bind(calls)
	.bind(w.id)
	.execute(w.store.pool.fixture_driver())
	.await
	.unwrap();
	let mut refusal = w.refusal(RefusalCheck::Policy, true);
	refusal.call.as_mut().unwrap().tool_call = Some(aidash_domain::tool::RefusedToolCall {
		name: "task_delegate".into(),
		arguments: json!({"task_id":"correct"}),
	});
	w.pause(&refusal).await;
	let event: Value = sqlx::query_scalar(
		"SELECT data FROM events WHERE kind = 'run.authorization_blocked' AND data->>'run_id' = $1",
	)
	.bind(w.id.to_string())
	.fetch_one(w.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(event.get("call").is_none(), "{event}");
	assert!(!event.to_string().contains("correct"), "{event}");
	w.decline(&w.key, "alice").await.unwrap();
	let (tool, input): (String, Value) =
		sqlx::query_as("SELECT tool, input FROM invocations WHERE idempotency_key = $1")
			.bind(&w.key)
			.fetch_one(w.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(tool, "task_delegate");
	assert_eq!(input, json!({"task_id":"correct"}));
}

#[rstest]
#[tokio::test]
async fn a_legacy_pause_does_not_journal_a_different_tool(
	#[future] database: DatabaseFixture,
	run: Record,
) {
	let w = world(database.await, run).await;
	let mut refusal = w.refusal(RefusalCheck::Policy, true);
	refusal.tool = "workspace_read".into();
	w.pause(&refusal).await;
	let mut tx = w.store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let result = w
		.store
		.decline_call_in(&mut tx, w.id, &w.key, "alice")
		.await;
	assert!(matches!(result, Err(Error::Conflict(_))));
	drop(tx);
	assert!(w.row().await.is_none());
	assert_eq!(w.run().await.control, crate::domain::RunControl::Paused);
}

#[rstest]
#[tokio::test]
async fn a_stale_key_or_a_second_decline_answers_conflict_and_changes_nothing(
	#[future] database: DatabaseFixture,
	run: Record,
) {
	let w = world(database.await, run).await;
	w.pause(&w.refusal(RefusalCheck::Policy, true)).await;
	let stale = format!("{}:{}:9", w.id, EPOCH);
	assert!(matches!(
		w.decline(&stale, "alice").await,
		Err(Error::Conflict(_))
	));
	assert!(w.row().await.is_none());
	assert!(w.recovery().await.get("paused_call").is_some());
	w.decline(&w.key, "alice").await.unwrap();
	// Declined once: the Run is no longer paused on it.
	assert!(matches!(
		w.decline(&w.key, "alice").await,
		Err(Error::Conflict(_))
	));
}

#[rstest]
#[tokio::test]
async fn a_receiver_records_that_the_home_declined_without_naming_anyone(
	#[future] database: DatabaseFixture,
	run: Record,
) {
	let w = world(database.await, run).await;
	w.pause(&w.refusal(RefusalCheck::RemoteAuthority, true))
		.await;
	w.decline(&w.key, "home").await.unwrap();
	let event: Value = sqlx::query_scalar(
		"SELECT data FROM events WHERE kind = 'run.call_declined' AND data->>'run_id' = $1",
	)
	.bind(w.id.to_string())
	.fetch_one(w.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(event["by"], "home");
	assert_eq!(event["kind"], "other_node");
}

#[rstest]
#[tokio::test]
async fn declining_keeps_what_resuming_changed_in_the_pending_state(
	#[future] database: DatabaseFixture,
	run: Record,
) {
	let w = world(database.await, run).await;
	w.pause(&w.refusal(RefusalCheck::Policy, true)).await;
	// A pause that also carries a context reason: resuming clears it and opens a new decision.
	sqlx::query(
		"UPDATE runs SET pending = jsonb_set(pending, '{recovery,context_reason}', \
		 '\"output_truncated\"'::jsonb, true) WHERE id = $1",
	)
	.bind(w.id)
	.execute(w.store.pool.fixture_driver())
	.await
	.unwrap();
	w.decline(&w.key, "alice").await.unwrap();
	let recovery = w.recovery().await;
	assert!(recovery.get("context_reason").is_none(), "{recovery}");
	assert_eq!(recovery["decision_cycle"], 1, "{recovery}");
	assert_eq!(
		recovery["declined_calls"],
		json!([{"key": w.key, "kind": "policy"}])
	);
}

#[rstest]
#[case::same_refusal("task_delegate", RefusalCheck::Policy, true)]
#[case::same_kind_different_check("task_delegate", RefusalCheck::CallUnapprovedDeny, true)]
#[case::different_tool("other_tool", RefusalCheck::Policy, false)]
#[case::different_kind("task_delegate", RefusalCheck::Catalog, false)]
#[tokio::test]
async fn standing_decline_only_answers_matching_later_refusals(
	#[future] database: DatabaseFixture,
	run: Record,
	#[case] tool: &str,
	#[case] check: RefusalCheck,
	#[case] matches: bool,
) {
	let mut w = world(database.await, run).await;
	w.pause(&w.refusal(RefusalCheck::Policy, true)).await;
	let mut tx = w.store.pool.begin(&TEST_FIXTURE).await.unwrap();
	w.store
		.decline_call_in(&mut tx, w.id, &w.key, "alice", true)
		.await
		.unwrap();
	tx.commit().await.unwrap();

	// The model has produced a new response, with a distinct invocation key.
	let current = w.run().await;
	let mut recovery = current.recovery;
	recovery.declined_calls.clear();
	let state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			tool_calls: vec![ToolCall {
				id: "later".into(),
				name: tool.into(),
				arguments: json!({}),
			}],
			..Default::default()
		},
		response_epoch: EPOCH + 1,
		..Default::default()
	}));
	w.key = format!("{}:{}:0", w.id, EPOCH + 1);
	let mut fixture_tx = w.store.pool.fixture_driver().begin().await.unwrap();
	sqlx::query("SELECT set_config('aidash.input_ledger_worker', 'true', true)")
		.execute(&mut *fixture_tx)
		.await
		.unwrap();
	sqlx::query("UPDATE runs SET pending = $1, lease_owner = $2, lease_until = $3, ledger_worker_ready = true WHERE id = $4")
		.bind(run_fixtures::pending(state, recovery))
		.bind(w.worker)
		.bind(Utc::now() + Duration::minutes(1))
		.bind(w.id)
		.execute(&mut *fixture_tx)
		.await
		.unwrap();
	fixture_tx.commit().await.unwrap();
	let mut refusal = w.refusal(check, true);
	refusal.tool = tool.into();
	w.pause(&refusal).await;
	let run = w.run().await;
	if matches {
		assert_eq!(run.control, aidash_domain::RunControl::Active);
		assert_eq!(
			w.row().await,
			Some((
				"COMPLETED".into(),
				Some(json!({"error":RefusalKind::Policy.declined_sentence()}))
			))
		);
		let event: Value = sqlx::query_scalar(
			"SELECT data FROM events WHERE data->>'run_id' = $1 AND data->>'standing_decline' = 'true'",
		)
		.bind(w.id.to_string())
		.fetch_one(w.store.pool.fixture_driver())
		.await
		.unwrap();
		assert_eq!(event["tool"], tool);
		assert_eq!(event["call"], w.key);
		assert_eq!(event["by"], "alice");
	} else {
		assert_eq!(run.control, aidash_domain::RunControl::Paused);
		assert!(w.row().await.is_none());
		assert_eq!(w.recovery().await["paused_call"]["tool"], tool);
	}
}
