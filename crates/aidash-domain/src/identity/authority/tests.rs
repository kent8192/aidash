use super::*;
use rstest::{fixture, rstest};
use serde_json::json;
fn now() -> DateTime<Utc> {
	DateTime::from_timestamp(1_790_000_000, 0).unwrap()
}
#[rstest]
#[case::fresh("external",Duration::minutes(14),Ok(()))]
#[case::boundary(
	"external",
	Duration::minutes(15),
	Err(DashboardStatusFailure::Unavailable)
)]
#[case::stale(
	"external",
	Duration::minutes(16),
	Err(DashboardStatusFailure::Unavailable)
)]
#[case::google("google",Duration::days(365),Ok(()))]
#[case::future("external",Duration::minutes(-1),Ok(()))]
#[case::just_inside("external",Duration::minutes(15)-Duration::nanoseconds(1),Ok(()))]
fn provider_status_preserves_the_exact_freshness_boundary(
	#[case] issuer: &str,
	#[case] age: Duration,
	#[case] expected: Result<(), DashboardStatusFailure>,
) {
	assert_eq!(
		dashboard_status(
			Some((issuer.into(), Some(now() - age), None)),
			now(),
			"google"
		),
		expected
	);
}
#[rstest]
#[case::missing(None)]
#[case::unvalidated(Some(("external".into(),None,None)))]
#[case::disabled(Some(("external".into(),Some(now()),Some(now()))))]
#[case::disabled_google(Some(("google".into(),Some(now()),Some(now()))))]
fn missing_or_disabled_identity_is_denied_before_provider_freshness(
	#[case] validity: Option<IdentityValidity>,
) {
	assert_eq!(
		dashboard_status(validity, now(), "google"),
		Err(DashboardStatusFailure::Forbidden)
	);
}
#[fixture]
fn bundle() -> crate::policy::PolicyBundle {
	serde_json::from_value(json!({"tenant":"tenant","subjects":{"reader":{"kind":"user"},"agent":{"kind":"agent","delegated_by":"reader"}}})).unwrap()
}
#[rstest]
#[case::primary("reader", true)]
#[case::delegate("agent", true)]
#[case::missing("unknown", false)]
fn enabled_identity_requires_a_valid_present_subject(
	bundle: crate::policy::PolicyBundle,
	#[case] subject: &str,
	#[case] expected: bool,
) {
	assert_eq!(enabled(&bundle, subject), expected);
}
#[rstest]
#[case::parent("reader")]
#[case::delegate("agent")]
fn every_subject_in_a_delegated_chain_must_remain_enabled(
	mut bundle: crate::policy::PolicyBundle,
	#[case] disabled: &str,
) {
	bundle.subjects.get_mut(disabled).unwrap().enabled = false;
	assert!(!enabled(&bundle, "agent"));
}
#[rstest]
fn malformed_delegation_does_not_authorize_or_loop(mut bundle: crate::policy::PolicyBundle) {
	bundle.subjects.get_mut("reader").unwrap().delegated_by = Some("agent".into());
	assert!(!enabled(&bundle, "agent"));
}
