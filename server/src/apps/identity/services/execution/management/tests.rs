use super::RunManagement;
use crate::domain::{RecoveryState, RetryState, RunControl, RunInspection, RunMetadata, RunPhase};
use aidash_domain::semantic::{
	Failure,
	recovery::{Action, Recovery},
};
use rstest::rstest;
use uuid::Uuid;

fn inspection(
	phase: RunPhase,
	control: RunControl,
	cause: Option<Failure>,
	retry: bool,
) -> RunInspection {
	RunInspection {
		state_version: Default::default(),
		metadata: RunMetadata {
			id: Uuid::now_v7(),
			task_id: Uuid::now_v7(),
			workspace_id: Uuid::now_v7(),
			home_node: "home".into(),
			agent_id: "agent".into(),
			agent_version: "1.0.0".into(),
			phase,
			control,
			step: 1,
			revision: 1,
			observed_input_seq: 0,
			ledger_worker_ready: false,
			error: cause.map(|cause| cause.to_string()),
			lease_owner: None,
			lease_until: None,
			updated_at: chrono::Utc::now(),
		},
		state: None,
		recovery: Some(RecoveryState {
			semantic_reason: cause,
			retry: retry.then(|| RetryState {
				count: 1,
				at: chrono::Utc::now(),
			}),
			..Default::default()
		}),
		context: None,
		state_error: None,
	}
}

#[rstest]
#[case::invalidated(RunControl::Paused, Failure::Invalidated, false, &[Action::CreateFollowUpTask])]
#[case::retrying(RunControl::Active, Failure::Unavailable, true, &[Action::Wait])]
#[case::exhausted(RunControl::Paused, Failure::RetriesExhausted, false, &[Action::RetryUnchangedBinding, Action::CreateFollowUpTask])]
fn management_reports_the_exact_cause_and_its_authorized_actions(
	#[case] control: RunControl,
	#[case] cause: Failure,
	#[case] retry: bool,
	#[case] actions: &[Action],
) {
	// Act
	let view = RunManagement::from(inspection(RunPhase::Ready, control, Some(cause), retry));

	// Assert
	assert_eq!(view.semantic_reason, Some(cause));
	assert_eq!(
		view.semantic_recovery,
		Some(Recovery {
			cause,
			actions: actions.to_vec(),
		})
	);
	let body = serde_json::to_value(&view).unwrap();
	assert_eq!(body["semantic_recovery"]["cause"], serde_json::json!(cause));
}

#[rstest]
#[case::cancelled(RunPhase::Ready, RunControl::Cancelled, Some(Failure::Invalidated))]
#[case::failed(RunPhase::Failed, RunControl::Active, Some(Failure::Invalidated))]
#[case::healthy(RunPhase::Ready, RunControl::Active, None)]
fn management_offers_no_recovery_without_a_recoverable_semantic_stop(
	#[case] phase: RunPhase,
	#[case] control: RunControl,
	#[case] cause: Option<Failure>,
) {
	let view = RunManagement::from(inspection(phase, control, cause, false));
	assert_eq!(view.semantic_recovery, None);
	assert!(
		serde_json::to_value(&view)
			.unwrap()
			.get("semantic_recovery")
			.is_none()
	);
}
