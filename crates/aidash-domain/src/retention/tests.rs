use super::*;
use chrono::TimeZone;
use rstest::rstest;

fn at(hours: i64) -> DateTime<Utc> {
	Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap() + Duration::hours(hours)
}

fn periods(after_run_hours: u32, max_age_hours: u32) -> SnapshotPeriods {
	SnapshotPeriods {
		after_run_seconds: after_run_hours * 3_600,
		max_age_seconds: max_age_hours * 3_600,
	}
}

#[rstest]
// A Run that has not ended keeps its snapshots up to the maximum age.
#[case(72, None, 71, true)]
#[case(72, None, 72, false)]
// A Run waiting three days still has its snapshot under a seven-day cap.
#[case(7 * 24, None, 3 * 24, true)]
// After the Run ends, the post-run period applies.
#[case(72, Some(10), 33, true)]
#[case(72, Some(10), 34, false)]
// The cap applies even within the post-run period.
#[case(72, Some(70), 72, false)]
fn snapshot_contents_follow_their_run_with_an_absolute_cap(
	#[case] max_age_hours: u32,
	#[case] ended_hours: Option<i64>,
	#[case] now_hours: i64,
	#[case] retained: bool,
) {
	let periods = periods(24, max_age_hours);
	assert_eq!(
		periods.retains(at(0), ended_hours.map(at), at(now_hours)),
		retained
	);
}

#[rstest]
fn the_default_keeps_contents_a_day_after_the_run_and_seven_days_at_most() {
	let periods = RetentionRules::DEFAULT.document_snapshots;
	assert_eq!(periods.after_run_seconds, 86_400);
	assert_eq!(periods.max_age_seconds, 604_800);
	assert!(RetentionRules::DEFAULT.validate().is_ok());
}

#[rstest]
fn a_zero_period_purges_at_the_next_sweep() {
	let periods = periods(0, 0);
	assert!(!periods.retains(at(0), None, at(0)));
}

#[rstest]
#[case(MAX_PERIOD_SECONDS, true)]
#[case(MAX_PERIOD_SECONDS + 1, false)]
fn every_period_is_finite(#[case] seconds: u32, #[case] valid: bool) {
	let rules = RetentionRules {
		document_snapshots: SnapshotPeriods {
			after_run_seconds: seconds,
			max_age_seconds: 1,
		},
	};
	assert_eq!(rules.validate().is_ok(), valid);
}

#[rstest]
#[case(periods(12, 48), true)]
#[case(periods(24, 7 * 24), true)]
#[case(periods(25, 48), false)]
#[case(periods(12, 7 * 24 + 1), false)]
fn a_tenant_may_only_shorten_the_deployment_periods(
	#[case] tenant: SnapshotPeriods,
	#[case] accepted: bool,
) {
	let tenant = RetentionRules {
		document_snapshots: tenant,
	};
	assert_eq!(
		tenant.validate_for_tenant(&RetentionRules::DEFAULT).is_ok(),
		accepted
	);
}

#[rstest]
fn the_shorter_of_the_deployment_and_tenant_periods_applies() {
	let deployment = RetentionRules {
		document_snapshots: periods(24, 48),
	};
	// Stored before the Deployment shortened its maximum age.
	let tenant = RetentionRules {
		document_snapshots: periods(1, 7 * 24),
	};
	assert_eq!(deployment.effective(None), deployment);
	assert_eq!(
		deployment.effective(Some(&tenant)).document_snapshots,
		periods(1, 48)
	);
}

#[rstest]
#[case(Decider::Subject { tenant: "acme".into(), subject: "alice".into() }, "subject:acme/alice")]
fn deciders_are_recorded_with_who_acted(#[case] decider: Decider, #[case] recorded: &str) {
	assert_eq!(decider.recorded(), recorded);
}

#[rstest]
#[case(
	serde_json::json!({"kind": "subject", "tenant": "acme", "subject": "alice"}),
	"subject:acme/alice"
)]
#[case(
	serde_json::json!({"kind": "deployment_administrator", "identity": "6f0c3c1e-5a43-4b7e-9d2f-0e8b1a2c3d4e"}),
	"deployment-administrator:6f0c3c1e-5a43-4b7e-9d2f-0e8b1a2c3d4e"
)]
#[case(
	serde_json::json!({"kind": "deployment_administrator"}),
	"deployment-administrator"
)]
fn records_kept_before_adr_0102_stay_readable(
	#[case] stored: serde_json::Value,
	#[case] recorded: &str,
) {
	let decider: RecordedDecider = serde_json::from_value(stored).unwrap();
	assert_eq!(decider.recorded(), recorded);
}

#[test]
fn a_new_decision_is_recorded_as_it_was_stored_before() {
	let decider = Decider::Subject {
		tenant: "acme".into(),
		subject: "alice".into(),
	};
	assert_eq!(
		serde_json::to_value(RecordedDecider::from(decider.clone())).unwrap(),
		serde_json::to_value(decider).unwrap()
	);
}
