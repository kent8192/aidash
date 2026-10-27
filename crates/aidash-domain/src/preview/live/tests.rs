use super::*;
use chrono::TimeDelta;

fn now() -> DateTime<Utc> {
	DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

fn preview() -> LivePreview {
	LivePreview::starting(
		Uuid::nil(),
		"acme",
		Uuid::now_v7(),
		Uuid::now_v7(),
		3,
		3000,
		Limits::default(),
		now(),
	)
	.unwrap()
}

fn running() -> LivePreview {
	let mut preview = preview();
	preview.serving("10.0.0.7", now());
	preview
}

#[test]
fn defaults_are_fifteen_minutes_idle_and_two_hours_absolute() {
	let limits = Limits::default();
	assert_eq!(limits.idle_seconds, 900);
	assert_eq!(limits.absolute_seconds, 7200);
	assert!(Limits::new(900, 7200).is_ok());
}

#[test]
fn limits_reject_zero_reversed_and_absurd_values() {
	for (idle, absolute) in [(0, 7200), (900, 0), (7200, 900), (900, 86_401)] {
		assert!(Limits::new(idle, absolute).is_err(), "{idle}/{absolute}");
	}
}

#[test]
fn a_tenant_may_only_lower_the_deployment_ceilings() {
	let ceiling = Limits::new(1800, 7200).unwrap();
	let lowered = Limits::new(900, 3600)
		.unwrap()
		.lower_than(&ceiling)
		.unwrap();
	assert_eq!(lowered, Limits::new(900, 3600).unwrap());
	let raised = Limits::new(3600, 86_400)
		.unwrap()
		.lower_than(&ceiling)
		.unwrap();
	assert_eq!(raised, ceiling);
	assert!(Limits::new(300, 600).unwrap().within(&ceiling));
	assert!(!Limits::new(3600, 7200).unwrap().within(&ceiling));
}

#[test]
fn a_lowered_absolute_limit_shortens_idle_with_it() {
	let ceiling = Limits::new(900, 1200).unwrap();
	let tenant = Limits::new(600, 900).unwrap();
	assert_eq!(
		tenant.lower_than(&ceiling).unwrap(),
		Limits::new(600, 900).unwrap()
	);
}

#[test]
fn a_preview_must_declare_a_non_privileged_port() {
	assert!(
		LivePreview::starting(
			Uuid::nil(),
			"acme",
			Uuid::nil(),
			Uuid::nil(),
			1,
			80,
			Limits::default(),
			now()
		)
		.is_err()
	);
	assert!(
		LivePreview::starting(
			Uuid::nil(),
			"acme",
			Uuid::nil(),
			Uuid::nil(),
			1,
			1024,
			Limits::default(),
			now()
		)
		.is_ok()
	);
}

#[test]
fn a_start_is_not_serving_until_the_runner_confirms_the_port() {
	let preview = preview();
	assert_eq!(preview.state, State::Stopped);
	assert_eq!(preview.view(3), State::Stopped);
	assert!(!preview.admits(now()));
}

#[test]
fn a_serving_preview_admits_until_its_idle_limit() {
	let mut preview = running();
	assert_eq!(preview.view(3), State::Running);
	assert!(preview.admits(now()));
	assert!(!preview.admits(now() + TimeDelta::seconds(preview.limits.idle_seconds + 1)));
	assert_eq!(
		preview.expired(now() + TimeDelta::seconds(preview.limits.idle_seconds + 1)),
		Some(StopReason::Idle)
	);
	// A gateway request restarts the idle timer, inside the absolute limit.
	preview.touched(now() + TimeDelta::seconds(preview.limits.idle_seconds - 1));
	assert!(preview.admits(now() + TimeDelta::seconds(preview.limits.idle_seconds + 1)));
}

#[test]
fn the_absolute_limit_wins_over_a_fresh_idle_timer() {
	let mut preview = running();
	let absolute = preview.expires_at;
	preview.touched(absolute - TimeDelta::seconds(1));
	assert_eq!(preview.expired(absolute), Some(StopReason::Absolute));
	assert!(!preview.admits(absolute));
}

#[test]
fn a_newer_revision_marks_it_stale_but_it_keeps_serving_its_pinned_one() {
	let preview = running();
	assert_eq!(preview.view(4), State::Stale);
	assert_eq!(preview.revision, 3);
	assert_eq!(preview.state, State::Running);
	assert!(preview.admits(now()));
}

#[test]
fn a_stop_request_stops_admission_and_waits_for_the_pod_proof() {
	let mut preview = running();
	assert!(!preview.needs_termination());
	preview.stopping(StopReason::RunCancelled);
	assert_eq!(preview.state, State::Stopped);
	assert_eq!(preview.stop_reason, Some(StopReason::RunCancelled));
	assert!(!preview.admits(now()));
	assert!(preview.needs_termination());
	preview.stopped(StopReason::RunCancelled);
	assert!(!preview.needs_termination());
	assert_eq!(preview.stop_reason, Some(StopReason::RunCancelled));
}

#[test]
fn a_stop_reason_round_trips_through_its_persisted_name() {
	for reason in [
		StopReason::Stopped,
		StopReason::Replaced,
		StopReason::RunCancelled,
		StopReason::RunAuthorityRevoked,
		StopReason::TenantDeleted,
		StopReason::AreaCleanedUp,
		StopReason::Idle,
		StopReason::Absolute,
		StopReason::StartFailed,
	] {
		assert_eq!(StopReason::parse(reason.as_str()).unwrap(), reason);
	}
	assert!(StopReason::parse("finished").is_err());
	assert_eq!(State::parse("stale").unwrap(), State::Stale);
	assert!(State::parse("frozen").is_err());
}

#[test]
fn a_startup_log_tail_is_bounded_and_valid_utf8() {
	let log = "é".repeat(MAX_STARTUP_LOG_BYTES);
	let tail = startup_tail(&log);
	assert!(tail.len() <= MAX_STARTUP_LOG_BYTES);
	assert!(tail.chars().all(|character| character == 'é'));
	assert_eq!(startup_tail("short"), "short");
}
