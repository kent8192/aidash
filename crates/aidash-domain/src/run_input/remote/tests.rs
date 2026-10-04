use super::*;
use crate::{RunControl, RunPhase};
use rstest::{fixture, rstest};
use uuid::Uuid;

#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		home_node: "home".into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		phase: RunPhase::Thinking,
		control: RunControl::Active,
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	}
}
fn message(run: &RunMetadata, key: &str) -> Message {
	Message {
		id: Uuid::new_v4(),
		workspace_id: run.workspace_id,
		sender: "human".into(),
		content: "correction".into(),
		idempotency_key: Some(full_key("executor", run.task_id, key)),
		created_at: run.updated_at,
	}
}

#[rstest]
fn historical_inputs_keep_qualified_identity_and_stable_order(run: RunMetadata) {
	let mut first = message(&run, "first");
	first.id = Uuid::from_u128(1);
	let mut second = message(&run, "second");
	second.id = Uuid::from_u128(2);
	let batch = historical_batch("executor", &run, vec![second, first]).unwrap();
	assert_eq!(
		batch
			.iter()
			.map(|(key, _)| key.as_str())
			.collect::<Vec<_>>(),
		["first", "second"]
	);
	assert_eq!(batch[0].1.id, Uuid::from_u128(1));
}

#[rstest]
#[case("workspace", "historical run message workspace changed")]
#[case("key", "historical run message key changed")]
fn historical_inputs_reject_foreign_identity(
	run: RunMetadata,
	#[case] changed: &str,
	#[case] error: &str,
) {
	let mut input = message(&run, "key");
	match changed {
		"workspace" => input.workspace_id = Uuid::new_v4(),
		"key" => input.idempotency_key = Some(full_key("other-node", run.task_id, "key")),
		_ => panic!("unknown fixture"),
	}
	assert_eq!(
		historical_batch("executor", &run, vec![input]).unwrap_err(),
		Error::Conflict(error.into())
	);
}

#[rstest]
#[case("workspace")]
#[case("content")]
#[case("key")]
fn delivered_record_must_match_the_committed_correction(run: RunMetadata, #[case] changed: &str) {
	let mut input = message(&run, "key");
	match changed {
		"workspace" => input.workspace_id = Uuid::new_v4(),
		"content" => input.content = "changed".into(),
		"key" => input.idempotency_key = None,
		_ => panic!("unknown fixture"),
	}
	assert_eq!(
		validate_delivery("executor", &run, "key", "correction", &input),
		Err(Error::Conflict(
			"remote run message delivery changed".into()
		))
	);
}

#[rstest]
fn legacy_history_includes_only_corrections_for_this_run(run: RunMetadata) {
	let own = format!("human:{}:1", run.id);
	let subject = format!("subject-human:subject:{}:2", run.id);
	let other = format!("human:{}:3", Uuid::new_v4());
	let mut foreign = message(&run, &own);
	foreign.idempotency_key = Some(full_key("other-node", run.task_id, &own));
	let inputs = vec![
		message(&run, &other),
		message(&run, "ordinary"),
		message(&run, &subject),
		foreign,
		message(&run, &own),
	];
	let result = legacy_history("executor", &run, inputs);
	assert_eq!(result.len(), 2);
	assert!(result.iter().all(|item| {
		[
			full_key("executor", run.task_id, &own),
			full_key("executor", run.task_id, &subject),
		]
		.contains(item.idempotency_key.as_ref().unwrap())
	}));
}
