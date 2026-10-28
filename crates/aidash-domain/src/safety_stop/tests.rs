use super::*;
use rstest::rstest;

fn command(action: Action, revision: i64) -> Command {
	Command::new(
		"acme",
		Uuid::from_u128(1),
		action,
		revision,
		Uuid::from_u128(2),
		"runaway spend",
	)
	.unwrap()
}

#[rstest]
#[case(RunPhase::Completed)]
#[case(RunPhase::Failed)]
#[case(RunPhase::Cancelled)]
fn a_terminal_run_is_a_no_op_at_any_revision(#[case] phase: RunPhase) {
	for control in [
		RunControl::Active,
		RunControl::Paused,
		RunControl::Cancelled,
	] {
		for action in [Action::Pause, Action::Cancel] {
			assert_eq!(
				decide(phase, control, 3, &command(action, 0)),
				Decision::NoOp
			);
			assert_eq!(
				decide(phase, control, 3, &command(action, 3)),
				Decision::NoOp
			);
		}
	}
}

#[rstest]
#[case(Action::Pause)]
#[case(Action::Cancel)]
fn a_cancelled_control_is_never_downgraded_or_stale(#[case] action: Action) {
	for revision in [0, 3, 99] {
		assert_eq!(
			decide(
				RunPhase::Thinking,
				RunControl::Cancelled,
				3,
				&command(action, revision)
			),
			Decision::NoOp
		);
	}
}

#[rstest]
#[case(RunControl::Active, Action::Pause)]
#[case(RunControl::Active, Action::Cancel)]
#[case(RunControl::Paused, Action::Pause)]
#[case(RunControl::Paused, Action::Cancel)]
fn a_current_revision_is_accepted(#[case] control: RunControl, #[case] action: Action) {
	for phase in [
		RunPhase::Ready,
		RunPhase::Thinking,
		RunPhase::ToolCall,
		RunPhase::Waiting,
	] {
		assert_eq!(
			decide(phase, control, 4, &command(action, 4)),
			Decision::Accepted
		);
	}
}

#[rstest]
#[case(RunControl::Active)]
#[case(RunControl::Paused)]
fn an_intervening_control_decision_is_stale(#[case] control: RunControl) {
	assert_eq!(
		decide(RunPhase::Waiting, control, 5, &command(Action::Pause, 4)),
		Decision::Stale
	);
	assert_eq!(
		decide(RunPhase::Waiting, control, 5, &command(Action::Cancel, 6)),
		Decision::Stale
	);
}

#[rstest]
#[case("")]
#[case("   ")]
#[case("bell\u{7}")]
fn a_missing_or_malformed_reason_is_refused(#[case] reason: &str) {
	assert!(Command::new("acme", Uuid::nil(), Action::Pause, 0, Uuid::nil(), reason).is_err());
}

#[rstest]
fn the_reason_is_trimmed_and_bounded() {
	let ok = Command::new(
		"acme",
		Uuid::nil(),
		Action::Pause,
		0,
		Uuid::nil(),
		"  why  ",
	)
	.unwrap();
	assert_eq!(ok.reason, "why");
	let longest = "a".repeat(1_000);
	assert!(Command::new("acme", Uuid::nil(), Action::Pause, 0, Uuid::nil(), &longest).is_ok());
	let too_long = "a".repeat(1_001);
	assert!(
		Command::new(
			"acme",
			Uuid::nil(),
			Action::Pause,
			0,
			Uuid::nil(),
			&too_long
		)
		.is_err()
	);
}

#[rstest]
fn a_negative_revision_and_an_invalid_tenant_are_refused() {
	assert!(Command::new("acme", Uuid::nil(), Action::Pause, -1, Uuid::nil(), "x").is_err());
	assert!(Command::new("", Uuid::nil(), Action::Pause, 0, Uuid::nil(), "x").is_err());
}

#[rstest]
fn the_fingerprint_ignores_only_the_operation_id() {
	let base = command(Action::Pause, 1);
	let mut other_operation = base.clone();
	other_operation.operation_id = Uuid::from_u128(9);
	assert_eq!(base.fingerprint(), other_operation.fingerprint());
	for changed in [
		command(Action::Cancel, 1),
		command(Action::Pause, 2),
		Command {
			reason: "another reason".into(),
			..base.clone()
		},
		Command {
			run_id: Uuid::from_u128(7),
			..base.clone()
		},
		Command {
			tenant: "other".into(),
			..base.clone()
		},
	] {
		assert_ne!(base.fingerprint(), changed.fingerprint());
	}
}

#[rstest]
fn the_shared_bearer_has_one_scope_and_names_nobody() {
	let id = Uuid::from_u128(5);
	assert_eq!(Actor::SharedBearer.caller_key(), "shared_bearer");
	assert_eq!(Actor::SharedBearer.identity(), None);
	assert_eq!(Actor::Browser { identity: id }.identity(), Some(id));
	assert_ne!(
		Actor::Browser { identity: id }.caller_key(),
		Actor::SharedBearer.caller_key()
	);
	assert_eq!(Actor::SharedBearer.kind(), "shared_bearer");
	assert_eq!(Actor::Browser { identity: id }.kind(), "browser");
}

#[rstest]
fn actions_and_outcomes_round_trip_their_stored_text() {
	for action in [Action::Pause, Action::Cancel] {
		assert_eq!(Action::try_from(action.as_str()).unwrap(), action);
	}
	assert!(Action::try_from("resume").is_err());
	for outcome in [Outcome::Accepted, Outcome::NoOp] {
		assert_eq!(Outcome::try_from(outcome.as_str()).unwrap(), outcome);
	}
}
