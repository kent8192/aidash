use super::*;
use rstest::rstest;

fn at(text: &str) -> DateTime<Utc> {
	DateTime::parse_from_rfc3339(text)
		.unwrap()
		.with_timezone(&Utc)
}

fn cron(expression: &str, zone: &str) -> Trigger {
	Trigger::Cron {
		expression: expression.into(),
		time_zone: zone.into(),
	}
}

fn definition(trigger: Trigger) -> Definition {
	Definition {
		title: "Daily report".into(),
		request: "Summarize yesterday's merged pull requests.".into(),
		agent: EntityRef {
			id: "reporter".into(),
			version: "1.0.0".into(),
		},
		trigger,
		thread: ThreadMode::New,
		approval_mode: None,
		max_lateness_seconds: DEFAULT_MAX_LATENESS_SECONDS,
	}
}

#[rstest]
// 09:00 on weekdays in Tokyo is 00:00 UTC; Friday 2026-11-13 is next after Thursday.
#[case(
	"0 9 * * 1-5",
	"Asia/Tokyo",
	"2026-11-12T01:00:00Z",
	"2026-11-13T00:00:00Z"
)]
// Day 0 and day 7 are both Sunday, as in ordinary cron.
#[case("30 6 * * 0", "UTC", "2026-11-13T00:00:00Z", "2026-11-15T06:30:00Z")]
#[case("30 6 * * 7", "UTC", "2026-11-13T00:00:00Z", "2026-11-15T06:30:00Z")]
#[case("30 6 * * sun", "UTC", "2026-11-13T00:00:00Z", "2026-11-15T06:30:00Z")]
// With both day fields restricted, either one matching is enough: the 1st, or a Monday.
#[case("0 0 1 * mon", "UTC", "2026-11-13T00:00:00Z", "2026-11-16T00:00:00Z")]
#[case("*/20 * * * *", "UTC", "2026-11-13T10:41:12Z", "2026-11-13T11:00:00Z")]
#[case("5/15 * * * *", "UTC", "2026-11-13T10:21:00Z", "2026-11-13T10:35:00Z")]
#[case("0 12 1 jan *", "UTC", "2026-11-13T00:00:00Z", "2027-01-01T12:00:00Z")]
fn cron_fires_as_ordinary_cron(
	#[case] expression: &str,
	#[case] zone: &str,
	#[case] after: &str,
	#[case] expected: &str,
) {
	// Act
	let next = cron(expression, zone)
		.next_after(at(after), at(after))
		.unwrap();
	// Assert
	assert_eq!(next, Some(at(expected)));
}

#[rstest]
fn a_time_skipped_by_spring_forward_runs_once_right_after_the_gap() {
	// Arrange: New York skips 02:00-03:00 on 2026-03-08.
	let daily = cron("30 2 * * *", "America/New_York");
	let quarterly = cron("*/15 2 * * *", "America/New_York");
	// Act
	let on_the_day = daily
		.next_after(at("2026-03-08T00:00:00Z"), at("2026-03-08T05:00:00Z"))
		.unwrap()
		.unwrap();
	let next_day = daily
		.next_after(at("2026-03-08T00:00:00Z"), on_the_day)
		.unwrap()
		.unwrap();
	let first = quarterly
		.next_after(at("2026-03-08T00:00:00Z"), at("2026-03-08T05:00:00Z"))
		.unwrap()
		.unwrap();
	let after_first = quarterly
		.next_after(at("2026-03-08T00:00:00Z"), first)
		.unwrap()
		.unwrap();
	// Assert: 02:30 never exists that day, so it runs at 03:00 EDT (07:00Z);
	// four times inside the gap run once.
	assert_eq!(on_the_day, at("2026-03-08T07:00:00Z"));
	assert_eq!(next_day, at("2026-03-09T06:30:00Z"));
	assert_eq!(first, at("2026-03-08T07:00:00Z"));
	assert_eq!(after_first, at("2026-03-09T06:00:00Z"));
}

#[rstest]
fn a_time_repeated_by_fall_back_runs_once() {
	// Arrange: New York repeats 01:00-02:00 on 2026-11-01.
	let repeated = cron("30 1 * * *", "America/New_York");
	// Act
	let first = repeated
		.next_after(at("2026-11-01T00:00:00Z"), at("2026-11-01T04:00:00Z"))
		.unwrap()
		.unwrap();
	let second = repeated
		.next_after(at("2026-11-01T00:00:00Z"), first)
		.unwrap()
		.unwrap();
	// Assert: 01:30 EDT only; the next run is the following day.
	assert_eq!(first, at("2026-11-01T05:30:00Z"));
	assert_eq!(second, at("2026-11-02T06:30:00Z"));
}

#[rstest]
#[case("* * * *")]
#[case("0 0 * * * *")]
#[case("60 * * * *")]
#[case("0 24 * * *")]
#[case("0 0 0 * *")]
#[case("0 0 * 13 *")]
#[case("0 0 * * 8")]
#[case("5-1 * * * *")]
#[case("*/0 * * * *")]
#[case("0 0 30 feb *")]
fn bad_cron_expressions_are_refused(#[case] expression: &str) {
	// Act
	let result = cron(expression, "UTC").validate(60, at("2026-11-13T00:00:00Z"));
	// Assert
	assert!(matches!(result, Err(Error::Invalid(_))), "{expression}");
}

#[rstest]
#[case(cron("*/5 * * * *", "UTC"), false)]
#[case(cron("0,10 9 * * *", "UTC"), false)]
#[case(cron("*/15 * * * *", "UTC"), true)]
// A close pair that occurs once a year is found without waiting for it.
#[case(cron("0,5 9 1 1 *", "UTC"), false)]
#[case(cron("55 23,0 * * *", "UTC"), true)]
#[case(cron("50,58 23 * * *", "UTC"), false)]
// Across midnight, as if both days could match.
#[case(cron("58,2 23,0 31 * *", "UTC"), false)]
#[case(cron("0 9 * * *", "Europe/Berlin"), true)]
#[case(Trigger::Interval { every_seconds: 600 }, false)]
#[case(Trigger::Interval { every_seconds: 900 }, true)]
fn the_minimum_interval_bounds_every_gap(#[case] trigger: Trigger, #[case] accepted: bool) {
	// Act
	let result = trigger.validate(DEFAULT_MIN_INTERVAL_SECONDS, at("2026-11-13T00:00:00Z"));
	// Assert
	assert_eq!(result.is_ok(), accepted, "{result:?}");
}

#[rstest]
fn an_unknown_time_zone_is_refused() {
	// Act
	let result = cron("0 9 * * *", "Mars/Olympus").validate(900, at("2026-11-13T00:00:00Z"));
	// Assert
	assert!(matches!(result, Err(Error::Invalid(message)) if message.contains("time zone")));
}

#[rstest]
fn an_interval_counts_from_its_anchor() {
	// Arrange
	let trigger = Trigger::Interval {
		every_seconds: 3600,
	};
	let anchor = at("2026-11-13T00:10:00Z");
	// Act + Assert
	assert_eq!(
		trigger
			.next_after(anchor, at("2026-11-13T00:00:00Z"))
			.unwrap(),
		Some(anchor)
	);
	assert_eq!(
		trigger.next_after(anchor, anchor).unwrap(),
		Some(at("2026-11-13T01:10:00Z"))
	);
	assert_eq!(
		trigger
			.next_after(anchor, at("2026-11-13T05:59:59Z"))
			.unwrap(),
		Some(at("2026-11-13T06:10:00Z"))
	);
}

#[rstest]
fn ticks_missed_while_down_run_once_late_at_the_latest_missed_time() {
	// Arrange: hourly, down from 01:00 to 03:20; the stored next tick is 01:00.
	let timing = Timing {
		trigger: cron("0 * * * *", "UTC"),
		anchor: at("2026-11-13T00:00:00Z"),
		max_lateness_seconds: 3600,
	};
	// Act
	let claim = timing
		.claim(at("2026-11-13T01:00:00Z"), at("2026-11-13T03:20:00Z"))
		.unwrap()
		.unwrap();
	// Assert
	assert_eq!(
		claim,
		Claim {
			scheduled_at: at("2026-11-13T03:00:00Z"),
			late: false,
			next: Some(at("2026-11-13T04:00:00Z")),
		}
	);
}

#[rstest]
fn a_tick_later_than_the_maximum_lateness_is_skipped() {
	// Arrange
	let timing = Timing {
		trigger: cron("0 9 * * *", "UTC"),
		anchor: at("2026-11-13T00:00:00Z"),
		max_lateness_seconds: 600,
	};
	// Act
	let claim = timing
		.claim(at("2026-11-13T09:00:00Z"), at("2026-11-13T09:30:00Z"))
		.unwrap()
		.unwrap();
	// Assert
	assert!(claim.late);
	assert_eq!(claim.scheduled_at, at("2026-11-13T09:00:00Z"));
	assert_eq!(claim.next, Some(at("2026-11-14T09:00:00Z")));
}

#[rstest]
fn nothing_is_claimed_before_the_tick() {
	// Arrange
	let timing = Timing {
		trigger: Trigger::Interval { every_seconds: 900 },
		anchor: at("2026-11-13T00:00:00Z"),
		max_lateness_seconds: 600,
	};
	// Act + Assert
	assert_eq!(
		timing
			.claim(at("2026-11-13T00:15:00Z"), at("2026-11-13T00:14:59Z"))
			.unwrap(),
		None
	);
}

#[rstest]
fn jitter_is_hidden_bounded_and_stable() {
	// Arrange
	let ids: Vec<Uuid> = (0..200).map(|_| Uuid::new_v4()).collect();
	// Act + Assert
	for id in ids {
		let jitter = jitter_seconds(id);
		assert!((0..=MAX_JITTER_SECONDS).contains(&jitter));
		assert_eq!(jitter, jitter_seconds(id));
	}
}

#[rstest]
fn a_definition_is_checked_whole() {
	// Arrange
	let mut long = definition(cron("0 9 * * *", "UTC"));
	long.title = "x".repeat(TITLE_MAX + 1);
	let mut late = definition(cron("0 9 * * *", "UTC"));
	late.max_lateness_seconds = 10;
	let mut agentless = definition(cron("0 9 * * *", "UTC"));
	agentless.agent.version = String::new();
	// Act + Assert
	assert!(
		definition(cron("0 9 * * *", "UTC"))
			.validate(900, Utc::now())
			.is_ok()
	);
	for invalid in [long, late, agentless] {
		assert!(matches!(
			invalid.validate(900, Utc::now()),
			Err(Error::Invalid(_))
		));
	}
}

#[rstest]
fn a_definition_defaults_to_a_new_thread_and_the_registrants_mode() {
	// Act
	let parsed: Definition = serde_json::from_value(serde_json::json!({
		"title": "t", "request": "r", "agent": {"id": "a", "version": "1.0.0"},
		"trigger": {"kind": "interval", "every_seconds": 3600}
	}))
	.unwrap();
	// Assert
	assert_eq!(parsed.thread, ThreadMode::New);
	assert_eq!(parsed.approval_mode, None);
	assert_eq!(parsed.max_lateness_seconds, DEFAULT_MAX_LATENESS_SECONDS);
}

#[rstest]
fn limits_stay_within_the_ceilings_and_the_tier() {
	// Arrange
	let ceilings = Ceilings {
		max_schedules: 40,
		max_per_person: 5,
		min_interval_seconds: 1800,
	};
	// Act + Assert
	assert!(ceilings.permits(&Limits::default()).is_err());
	assert!(
		ceilings
			.permits(&Limits {
				max_schedules: 40,
				max_per_person: 5,
				min_interval_seconds: 1800,
			})
			.is_ok()
	);
	assert!(
		ceilings
			.permits(&Limits {
				max_schedules: 10,
				max_per_person: 5,
				min_interval_seconds: 900,
			})
			.is_err()
	);
	assert_eq!(
		ceilings.effective(Limits::default(), Some(12)),
		Limits {
			max_schedules: 12,
			max_per_person: 5,
			min_interval_seconds: 1800,
		}
	);
}

#[rstest]
#[case(49, 9, true)]
#[case(50, 0, false)]
#[case(10, 10, false)]
fn the_tenant_total_and_the_per_person_count_both_bound(
	#[case] total: i64,
	#[case] mine: i64,
	#[case] admitted: bool,
) {
	// Act
	let result = Limits::default().admit(total, mine);
	// Assert
	assert_eq!(result.is_ok(), admitted);
}

fn standing<'a>(halt: Option<Halt>, successor: Option<&'a str>) -> Standing<'a> {
	Standing {
		registrant: "alice",
		halt,
		successor,
	}
}

fn person(subject: &str, administers: bool) -> Requester<'_> {
	Requester {
		subject,
		administers,
	}
}

#[rstest]
fn only_the_registrant_edits_and_resumes_their_schedule() {
	// Act + Assert
	assert!(permit(person("alice", false), standing(None, None), Change::Edit).is_ok());
	assert!(permit(person("admin", true), standing(None, None), Change::Edit).is_err());
	assert!(permit(person("bob", false), standing(None, None), Change::Pause).is_err());
	assert!(permit(person("admin", true), standing(None, None), Change::Pause).is_ok());
	assert!(permit(person("admin", true), standing(None, None), Change::Delete).is_ok());
	assert_eq!(
		resumption(person("alice", false), standing(Some(Halt::Failures), None)).unwrap(),
		Resumption::Registrant
	);
	assert!(resumption(person("admin", true), standing(Some(Halt::Paused), None)).is_err());
	assert!(resumption(person("alice", false), standing(None, None)).is_err());
}

#[rstest]
fn a_schedule_whose_registrant_left_runs_again_only_as_its_successors_own() {
	// Arrange
	let waiting = standing(Some(Halt::RegistrantLeft), Some("carol"));
	// Act + Assert
	assert!(
		permit(
			person("admin", true),
			standing(Some(Halt::RegistrantLeft), None),
			Change::NameSuccessor
		)
		.is_ok()
	);
	assert!(
		permit(
			person("admin", true),
			standing(None, None),
			Change::NameSuccessor
		)
		.is_err()
	);
	assert!(
		permit(
			person("carol", false),
			standing(Some(Halt::RegistrantLeft), None),
			Change::NameSuccessor
		)
		.is_err()
	);
	assert_eq!(
		resumption(person("carol", false), waiting).unwrap(),
		Resumption::Successor
	);
	assert!(resumption(person("admin", true), waiting).is_err());
	assert!(resumption(person("alice", false), waiting).is_err());
	assert!(permit(person("alice", false), waiting, Change::Edit).is_err());
}

#[rstest]
#[case(Caller { root_person: Some("alice"), federated: false, scheduled: false }, true, Ok("alice"))]
#[case(Caller { root_person: Some("alice"), federated: false, scheduled: true }, false, Ok("alice"))]
#[case(Caller { root_person: Some("alice"), federated: false, scheduled: true }, true, Err(()))]
#[case(Caller { root_person: Some("alice"), federated: true, scheduled: false }, false, Err(()))]
#[case(Caller { root_person: None, federated: false, scheduled: false }, true, Err(()))]
fn agents_register_schedules_only_for_a_person_and_never_from_a_scheduled_chain(
	#[case] caller: Caller<'static>,
	#[case] changes: bool,
	#[case] expected: std::result::Result<&str, ()>,
) {
	// Act
	let result = agent_registrant(caller, changes).map_err(|_| ());
	// Assert
	assert_eq!(result, expected);
}

#[rstest]
fn three_failed_runs_in_a_row_pause_and_a_completion_resets() {
	// Act + Assert
	assert_eq!(after_settled(0, TickStatus::Failed), (1, false));
	assert_eq!(after_settled(2, TickStatus::Failed), (3, true));
	assert_eq!(after_settled(2, TickStatus::Cancelled), (2, false));
	assert_eq!(after_settled(2, TickStatus::Completed), (0, false));
}

#[rstest]
fn halts_and_tick_statuses_round_trip() {
	for halt in [
		Halt::Paused,
		Halt::Failures,
		Halt::RegistrantLeft,
		Halt::AgentVersionUnusable,
		Halt::ThreadDeleted,
		Halt::TenantDeleted,
		Halt::IntervalTooShort,
	] {
		assert_eq!(Halt::parse(halt.as_str()).unwrap(), halt);
		assert_eq!(serde_json::to_value(halt).unwrap(), halt.as_str());
	}
	for status in [
		TickStatus::Started,
		TickStatus::Completed,
		TickStatus::Failed,
		TickStatus::Cancelled,
		TickStatus::Skipped,
	] {
		assert_eq!(TickStatus::parse(status.as_str()).unwrap(), status);
	}
	assert!(!Halt::Paused.notifies());
	assert!(Halt::RegistrantLeft.notifies());
}

#[rstest]
fn a_tick_sooner_than_the_minimum_interval_after_the_last_is_too_soon() {
	// Arrange: a daylight-saving change brought two times an hour apart within a minute.
	let previous = at("2026-03-08T06:59:00Z");
	// Act + Assert
	assert!(too_soon(Some(previous), at("2026-03-08T07:00:00Z"), 900));
	assert!(!too_soon(
		Some(previous),
		at("2026-03-08T07:14:00Z") + Duration::minutes(1),
		900
	));
	assert!(!too_soon(None, at("2026-03-08T07:00:00Z"), 900));
}
