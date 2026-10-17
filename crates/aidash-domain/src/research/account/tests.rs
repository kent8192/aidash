use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn profile() -> AccountProfile {
	serde_json::from_value(serde_json::json!({
		"account_id":"search-account",
		"agreement_reference":"approved-contract-record",
		"verified_by":"deployment_administrator",
		"verified_at":"2026-09-01T00:00:00Z",
		"review_after":"2026-10-01T00:00:00Z",
		"storage_permitted":true,
		"query_training_prohibited":true,
		"evaluation_permitted":true,
		"retention_days":90,
		"deletion_after_termination_days":30,
		"credential_env":"AIDASH_SECRET_BRAVE_SEARCH",
		"price_effective_at":"2026-09-01T00:00:00Z",
		"price_review_after":"2026-10-01T00:00:00Z",
		"request_price_micro_usd":5000,
		"monthly_base_fee_micro_usd":0,
		"monthly_limit_micro_usd":20_000_000
	}))
	.unwrap()
}

fn at(value: &str) -> DateTime<Utc> {
	value.parse().unwrap()
}

#[rstest]
fn admission_requires_current_complete_terms(mut profile: AccountProfile) {
	let now = at("2026-09-25T00:00:00Z");
	assert!(profile.validate_at(now).is_ok());
	profile.query_training_prohibited = false;
	assert!(profile.validate_at(now).is_err());
	profile.query_training_prohibited = true;
	profile.monthly_base_fee_micro_usd = 20_000_000;
	assert!(profile.validate_at(now).is_err());
	profile.monthly_base_fee_micro_usd = 0;
	profile.price_review_after = at("2026-09-24T00:00:00Z");
	assert!(profile.validate_at(now).is_err());
	profile.price_review_after = at("2026-10-01T00:00:00Z");
	profile.credential_env = "INLINE_SECRET".into();
	assert!(profile.validate_at(now).is_err());
	profile.credential_env = "AIDASH_SECRET_BRAVE_SEARCH".into();
	assert!(profile.validate_at(at("2026-10-02T00:00:00Z")).is_err());
}

#[rstest]
fn a_terminated_account_admits_nothing_from_its_end(mut profile: AccountProfile) {
	profile.terminated_at = Some(at("2026-09-25T00:00:00Z"));
	assert!(profile.validate_at(at("2026-09-24T23:59:59Z")).is_ok());
	assert!(profile.validate_at(at("2026-09-25T00:00:00Z")).is_err());
	assert_eq!(
		profile.termination(),
		Some(Termination {
			account_id: "search-account".into(),
			terminated_at: at("2026-09-25T00:00:00Z"),
			deletion_after_termination_days: 30,
		})
	);
}

#[rstest]
fn terms_without_an_end_still_parse_and_have_no_termination(profile: AccountProfile) {
	assert_eq!(profile.terminated_at, None);
	assert_eq!(profile.termination(), None);
	assert!(
		serde_json::to_value(&profile)
			.unwrap()
			.get("terminated_at")
			.is_none()
	);
}

#[rstest]
#[case::recorded_terms(90, None, None, "2026-12-30T00:00:00Z")]
#[case::shorter_current_terms(90, Some(30), None, "2026-10-31T00:00:00Z")]
#[case::longer_current_terms_never_extend(30, Some(90), None, "2026-10-31T00:00:00Z")]
#[case::termination_first(90, Some(90), Some(("2026-10-05T00:00:00Z", 7)), "2026-10-12T00:00:00Z")]
#[case::retention_before_termination(5, None, Some(("2026-10-05T00:00:00Z", 30)), "2026-10-06T00:00:00Z")]
fn the_basis_ends_at_the_earliest_permitted_moment(
	#[case] recorded: u16,
	#[case] current: Option<u16>,
	#[case] termination: Option<(&str, u16)>,
	#[case] expected: &str,
) {
	let termination = termination.map(|(terminated_at, days)| Termination {
		account_id: "search-account".into(),
		terminated_at: at(terminated_at),
		deletion_after_termination_days: days,
	});
	assert_eq!(
		basis_ends_at(
			at("2026-10-01T00:00:00Z"),
			recorded,
			current,
			termination.as_ref()
		),
		at(expected)
	);
}

#[rstest]
fn amounts_must_fit_the_recorded_charge_range(mut profile: AccountProfile) {
	let now = at("2026-09-25T00:00:00Z");
	profile.request_price_micro_usd = MAX_MICRO_USD;
	profile.monthly_limit_micro_usd = MAX_MICRO_USD;
	assert!(profile.validate_at(now).is_ok());
	// A price beyond the recorded range needs a limit beyond it as well.
	profile.request_price_micro_usd = MAX_MICRO_USD + 1;
	profile.monthly_limit_micro_usd = u64::MAX;
	assert!(profile.validate_at(now).is_err());
	profile.request_price_micro_usd = 5_000;
	assert!(profile.validate_at(now).is_err());
}

#[rstest]
fn the_monthly_limit_is_checked_before_one_more_request(mut profile: AccountProfile) {
	profile.monthly_base_fee_micro_usd = 1_000;
	profile.monthly_limit_micro_usd = 11_000;
	assert!(profile.admits(5_000));
	assert!(!profile.admits(5_001));
	assert_eq!(
		profile.attribution(),
		CostAttribution {
			account_id: "search-account".into(),
			price_micro_usd: 5_000
		}
	);
}

#[rstest]
fn months_start_at_utc_midnight_on_the_first() {
	assert_eq!(
		month_start(at("2026-09-25T13:14:15Z")),
		at("2026-09-01T00:00:00Z")
	);
}
