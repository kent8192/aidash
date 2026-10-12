use super::{Action, Recovery};
use crate::semantic::Failure;
use rstest::rstest;

#[rstest]
#[case::pending(Failure::Pending, false, &[Action::Wait])]
#[case::scheduled_outage(Failure::Unavailable, true, &[Action::Wait])]
#[case::unscheduled_outage(Failure::Unavailable, false, &[Action::RetryUnchangedBinding, Action::CreateFollowUpTask])]
#[case::exhausted(Failure::RetriesExhausted, false, &[Action::RetryUnchangedBinding, Action::CreateFollowUpTask])]
#[case::authority(Failure::Authority, false, &[Action::RetryUnchangedBinding, Action::CreateFollowUpTask])]
#[case::configuration(Failure::Configuration, false, &[Action::RetryUnchangedBinding, Action::CreateFollowUpTask])]
#[case::allowance(Failure::Allowance, false, &[Action::RetryUnchangedBinding, Action::CreateFollowUpTask])]
#[case::provider_contract(Failure::ProviderContract, false, &[Action::RetryUnchangedBinding, Action::CreateFollowUpTask])]
#[case::context_budget(Failure::ContextBudget, false, &[Action::CreateFollowUpTask])]
#[case::invalidated(Failure::Invalidated, false, &[Action::CreateFollowUpTask])]
#[case::invalidated_with_a_stale_retry(Failure::Invalidated, true, &[Action::CreateFollowUpTask])]
fn each_cause_offers_only_its_authorized_actions(
	#[case] cause: Failure,
	#[case] retry_scheduled: bool,
	#[case] expected: &[Action],
) {
	// Act
	let recovery = Recovery::new(cause, retry_scheduled);

	// Assert
	assert_eq!(recovery.cause, cause);
	assert_eq!(recovery.actions, expected);
}

#[test]
fn an_invalidated_run_is_never_offered_a_resume() {
	for retry_scheduled in [false, true] {
		let recovery = Recovery::new(Failure::Invalidated, retry_scheduled);
		assert!(
			!recovery
				.actions
				.iter()
				.any(|action| matches!(action, Action::Wait | Action::RetryUnchangedBinding)),
			"continuing would reuse context that a source change invalidated"
		);
	}
}

#[test]
fn recovery_serializes_with_stable_snake_case_names() {
	let value = serde_json::to_value(Recovery::new(Failure::Invalidated, false)).unwrap();
	assert_eq!(
		value,
		serde_json::json!({"cause":"invalidated","actions":["create_follow_up_task"]})
	);
	let value = serde_json::to_value(Recovery::new(Failure::Pending, false)).unwrap();
	assert_eq!(
		value,
		serde_json::json!({"cause":"pending","actions":["wait"]})
	);
	assert!(
		serde_json::from_value::<Recovery>(
			serde_json::json!({"cause":"pending","actions":["wait"],"label":"resume"})
		)
		.is_err()
	);
}
