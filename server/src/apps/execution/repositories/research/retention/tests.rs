use super::*;
use rstest::rstest;

fn read(values: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
	move |key| {
		values
			.iter()
			.find(|(name, _)| *name == key)
			.map(|(_, value)| (*value).to_owned())
	}
}

#[rstest]
fn the_deployment_keeps_the_offering_defaults_unless_set() {
	assert_eq!(
		deployment_rules(read(&[])).unwrap(),
		RetentionRules::DEFAULT
	);
}

#[rstest]
#[case(&[(AFTER_RUN, "0"), (MAX_AGE, "3600")], 0, 3_600)]
#[case(&[(MAX_AGE, "315360000")], 86_400, 315_360_000)]
fn whoever_runs_the_deployment_sets_any_finite_period(
	#[case] values: &'static [(&'static str, &'static str)],
	#[case] after_run: u32,
	#[case] max_age: u32,
) {
	let rules = deployment_rules(read(values)).unwrap();
	assert_eq!(rules.document_snapshots.after_run_seconds, after_run);
	assert_eq!(rules.document_snapshots.max_age_seconds, max_age);
}

#[rstest]
#[case(&[(AFTER_RUN, "-1")])]
#[case(&[(AFTER_RUN, "never")])]
#[case(&[(MAX_AGE, "315360001")])]
fn a_period_that_is_not_finite_and_bounded_refuses_to_start(
	#[case] values: &'static [(&'static str, &'static str)],
) {
	assert!(matches!(
		deployment_rules(read(values)),
		Err(Error::Invalid(_))
	));
}
