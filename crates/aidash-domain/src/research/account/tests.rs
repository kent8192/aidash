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
