use super::*;
use rstest::rstest;

fn budget(
	soft: Option<i64>,
	hard: Option<i64>,
	on_hard: OnHardLimit,
	period: Period,
) -> Definition {
	Definition {
		scope: Scope::Tenant,
		period,
		soft_limit_nanocredits: soft,
		hard_limit_nanocredits: hard,
		on_hard_limit: on_hard,
		recipients: soft
			.map(|_| vec!["owner@example.com".into()])
			.unwrap_or_default(),
	}
}

fn at(text: &str) -> DateTime<Utc> {
	text.parse().unwrap()
}

#[rstest]
#[case::no_limit(None, None, None)]
#[case::zero(None, Some(0), None)]
#[case::soft_not_below_hard(Some(10), Some(10), None)]
#[case::soft_only(Some(10), None, Some(()))]
#[case::hard_only(None, Some(10), Some(()))]
#[case::both(Some(5), Some(10), Some(()))]
fn a_budget_needs_a_positive_limit_and_soft_below_hard(
	#[case] soft: Option<i64>,
	#[case] hard: Option<i64>,
	#[case] valid: Option<()>,
) {
	// Act
	let result = budget(soft, hard, OnHardLimit::Stop, Period::Monthly).validate();
	// Assert
	assert_eq!(result.is_ok(), valid.is_some(), "{result:?}");
}

#[rstest]
#[case::missing_at("owner.example.com")]
#[case::empty_local("@example.com")]
#[case::space("own er@example.com")]
#[case::two_ats("a@b@example.com")]
fn soft_limit_mail_needs_plain_addresses(#[case] address: &str) {
	// Arrange
	let mut definition = budget(Some(5), None, OnHardLimit::Stop, Period::Monthly);
	definition.recipients = vec![address.into()];
	// Act / Assert
	assert!(definition.validate().is_err());
}

#[test]
fn a_soft_limit_without_recipients_is_refused() {
	// Arrange
	let mut definition = budget(Some(5), None, OnHardLimit::Stop, Period::Monthly);
	definition.recipients.clear();
	// Act / Assert
	assert!(definition.validate().is_err());
}

#[test]
fn a_monthly_window_starts_on_the_first_and_resets_on_the_next_first() {
	// Arrange
	let now = at("2026-12-17T09:30:00Z");
	// Act / Assert
	assert_eq!(
		Period::Monthly.window_start(at("2026-01-01T00:00:00Z"), now),
		at("2026-12-01T00:00:00Z")
	);
	assert_eq!(
		Period::Monthly.next_reset(now),
		Some(at("2027-01-01T00:00:00Z"))
	);
	assert_eq!(
		Period::Total.window_start(at("2026-03-04T05:06:07Z"), now),
		at("2026-03-04T05:06:07Z")
	);
	assert_eq!(Period::Total.next_reset(now), None);
}

#[rstest]
#[case::under(4, Standing::Under)]
#[case::soft(5, Standing::Soft)]
#[case::hard(10, Standing::Hard)]
fn standing_follows_the_limits(#[case] spent: i64, #[case] expected: Standing) {
	// Act / Assert
	assert_eq!(
		budget(Some(5), Some(10), OnHardLimit::Stop, Period::Monthly).standing(spent),
		expected
	);
}

#[test]
fn a_stop_wins_over_a_sleep_and_the_latest_reset_wins_among_sleeps() {
	// Arrange
	let now = at("2026-12-17T09:30:00Z");
	let monthly = budget(None, Some(10), OnHardLimit::Sleep, Period::Monthly);
	let total = budget(None, Some(10), OnHardLimit::Sleep, Period::Total);
	let stop = budget(None, Some(10), OnHardLimit::Stop, Period::Monthly);
	let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
	// Act
	let open = gate([(a, &monthly, 9)], now);
	let monthly_sleep = gate([(a, &monthly, 10)], now);
	let never = gate([(a, &monthly, 10), (b, &total, 11)], now);
	let stopped = gate([(a, &monthly, 10), (c, &stop, 10)], now);
	// Assert
	assert_eq!(open, Gate::Open);
	assert_eq!(
		monthly_sleep,
		Gate::Sleep {
			budget: a,
			until: Some(at("2027-01-01T00:00:00Z"))
		}
	);
	assert_eq!(
		never,
		Gate::Sleep {
			budget: b,
			until: None
		}
	);
	assert_eq!(stopped, Gate::Stop { budget: c });
}

#[test]
fn scopes_round_trip_through_their_stored_parts() {
	// Arrange
	let duty = Uuid::now_v7();
	for scope in [
		Scope::Tenant,
		Scope::Node {
			node: "node-a".into(),
		},
		Scope::Duty { duty_id: duty },
		Scope::Person {
			subject: "alice".into(),
		},
	] {
		// Act
		let parsed = Scope::from_parts(scope.kind(), &scope.key()).unwrap();
		// Assert
		assert_eq!(parsed, scope);
	}
}
