use super::*;
use rstest::{fixture, rstest};

const SEARCH: &[Operation] = &[Operation::Search];

#[fixture]
fn terms() -> AccountTerms {
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
		"price_effective_at":"2026-09-01T00:00:00Z",
		"price_review_after":"2026-10-01T00:00:00Z",
		"search_price_micro_usd":5000,
		"monthly_base_fee_micro_usd":0,
		"monthly_limit_micro_usd":20_000_000
	}))
	.unwrap()
}

fn at(value: &str) -> DateTime<Utc> {
	value.parse().unwrap()
}

#[rstest]
fn complete_terms_are_required_whatever_the_date(mut terms: AccountTerms) {
	assert!(terms.validate(SEARCH).is_ok());
	terms.query_training_prohibited = false;
	assert!(terms.validate(SEARCH).is_err());
	terms.query_training_prohibited = true;
	terms.monthly_base_fee_micro_usd = 20_000_000;
	assert!(terms.validate(SEARCH).is_err());
	terms.monthly_base_fee_micro_usd = 0;
	terms.price_review_after = terms.price_effective_at;
	assert!(terms.validate(SEARCH).is_err());
}

#[rstest]
fn the_key_reference_is_not_part_of_the_terms(terms: AccountTerms) {
	let mut value = serde_json::to_value(terms).unwrap();
	value["credential_env"] = "AIDASH_SECRET_BRAVE_SEARCH".into();
	assert!(serde_json::from_value::<AccountTerms>(value).is_err());
}

#[rstest]
fn each_implemented_charged_operation_has_exactly_one_price(mut terms: AccountTerms) {
	// A searching provider needs a search price and must not name a fetch one.
	terms.fetch_price_micro_usd = Some(1_000);
	assert!(terms.validate(SEARCH).is_err());
	assert!(
		terms
			.validate(&[Operation::Search, Operation::Fetch, Operation::Find])
			.is_ok()
	);
	terms.search_price_micro_usd = None;
	assert!(terms.validate(&[Operation::Fetch]).is_ok());
	assert!(terms.validate(SEARCH).is_err());
}

#[rstest]
fn a_free_request_is_still_attributed(mut terms: AccountTerms) {
	terms.search_price_micro_usd = Some(0);
	terms.monthly_limit_micro_usd = 0;
	assert!(terms.validate(SEARCH).is_ok());
	assert!(terms.admits(Operation::Search, 0));
	assert_eq!(
		terms.attribution(Operation::Search),
		Some(CostAttribution {
			account_id: "search-account".into(),
			price_micro_usd: 0
		})
	);
}

#[rstest]
fn lapsed_terms_are_not_current(terms: AccountTerms) {
	assert!(terms.current_at(at("2026-09-25T00:00:00Z")).is_ok());
	assert!(terms.current_at(at("2026-08-31T00:00:00Z")).is_err());
	assert!(terms.current_at(at("2026-10-02T00:00:00Z")).is_err());
}

#[rstest]
fn amounts_must_fit_the_recorded_charge_range(mut terms: AccountTerms) {
	terms.search_price_micro_usd = Some(MAX_MICRO_USD);
	terms.monthly_limit_micro_usd = MAX_MICRO_USD;
	assert!(terms.validate(SEARCH).is_ok());
	// A price beyond the recorded range needs a limit beyond it as well.
	terms.search_price_micro_usd = Some(MAX_MICRO_USD + 1);
	terms.monthly_limit_micro_usd = u64::MAX;
	assert!(terms.validate(SEARCH).is_err());
	terms.search_price_micro_usd = Some(5_000);
	assert!(terms.validate(SEARCH).is_err());
}

#[rstest]
fn the_monthly_limit_is_checked_before_one_more_request(mut terms: AccountTerms) {
	terms.monthly_base_fee_micro_usd = 1_000;
	terms.monthly_limit_micro_usd = 11_000;
	assert!(terms.admits(Operation::Search, 5_000));
	assert!(!terms.admits(Operation::Search, 5_001));
	// An operation without a price is never admitted or attributed.
	assert!(!terms.admits(Operation::Fetch, 0));
	assert_eq!(terms.attribution(Operation::Fetch), None);
	assert_eq!(terms.request_allowance_micro_usd(), 10_000);
}

#[rstest]
fn months_start_at_utc_midnight_on_the_first() {
	assert_eq!(
		month_start(at("2026-09-25T13:14:15Z")),
		at("2026-09-01T00:00:00Z")
	);
}
