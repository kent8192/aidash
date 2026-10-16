use super::*;
use crate::apps::execution::serializers::research_retention::ResearchRetentionSettings;
use rstest::rstest;

fn settings(value: serde_json::Value) -> ResearchRetentionSettings {
	serde_json::from_value(value).unwrap()
}

#[rstest]
fn the_deployment_keeps_the_offering_defaults_unless_set() {
	assert_eq!(
		settings(serde_json::json!({})).rules().unwrap(),
		RetentionRules::DEFAULT
	);
}

#[rstest]
#[case(serde_json::json!({"snapshot_after_run_seconds": 0, "snapshot_max_age_seconds": 3600}), 0, 3_600)]
#[case(serde_json::json!({"snapshot_max_age_seconds": 315_360_000}), 86_400, 315_360_000)]
fn whoever_runs_the_deployment_sets_any_finite_period(
	#[case] value: serde_json::Value,
	#[case] after_run: u32,
	#[case] max_age: u32,
) {
	let rules = settings(value).rules().unwrap();
	assert_eq!(rules.document_snapshots.after_run_seconds, after_run);
	assert_eq!(rules.document_snapshots.max_age_seconds, max_age);
}

#[rstest]
fn a_period_beyond_ten_years_refuses_to_start() {
	let periods = settings(serde_json::json!({"snapshot_max_age_seconds": 315_360_001}));
	assert!(matches!(periods.rules(), Err(crate::Error::Invalid(_))));
	use reinhardt::conf::settings::{fragment::SettingsValidation, profile::Profile};
	assert!(periods.validate(&Profile::Development).is_err());
}

/// A period that is not a non-negative whole number of seconds is refused
/// when the settings load.
#[rstest]
#[case(serde_json::json!({"snapshot_after_run_seconds": -1}))]
#[case(serde_json::json!({"snapshot_after_run_seconds": "never"}))]
fn a_period_that_is_not_finite_refuses_to_load(#[case] value: serde_json::Value) {
	assert!(serde_json::from_value::<ResearchRetentionSettings>(value).is_err());
}
