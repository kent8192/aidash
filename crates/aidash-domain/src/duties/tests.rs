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
		mission: "Summarize yesterday's merged pull requests.".into(),
		procedure: String::new(),
		agent: EntityRef {
			id: "reporter".into(),
			version: "1.0.0".into(),
		},
		trigger: Some(trigger),
		approval_mode: None,
		max_lateness_seconds: DEFAULT_MAX_LATENESS_SECONDS,
		pause_after_failures: DEFAULT_PAUSE_AFTER_FAILURES,
		router: None,
		watcher: None,
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
	let result = cron(expression, "UTC").validate(at("2026-11-13T00:00:00Z"));
	// Assert
	assert!(matches!(result, Err(Error::Invalid(_))), "{expression}");
}

/// There is no minimum gap between fire times (#1012): a Budget, not a
/// count, bounds what a Duty spends.
#[rstest]
#[case(cron("* * * * *", "UTC"))]
#[case(cron("0,1 9 * * *", "UTC"))]
#[case(Trigger::Interval { every_seconds: 1 })]
fn any_frequency_is_accepted(#[case] trigger: Trigger) {
	// Act
	let result = trigger.validate(at("2026-11-13T00:00:00Z"));
	// Assert
	assert!(result.is_ok(), "{result:?}");
}

#[rstest]
fn an_unknown_time_zone_is_refused() {
	// Act
	let result = cron("0 9 * * *", "Mars/Olympus").validate(at("2026-11-13T00:00:00Z"));
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
		trigger: Some(cron("0 * * * *", "UTC")),
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
		trigger: Some(cron("0 9 * * *", "UTC")),
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
		trigger: Some(Trigger::Interval { every_seconds: 900 }),
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
	let mut negative = definition(cron("0 9 * * *", "UTC"));
	negative.pause_after_failures = -1;
	let mut nul = definition(cron("0 9 * * *", "UTC"));
	nul.procedure = "check\0".into();
	// Act + Assert
	assert!(
		definition(cron("0 9 * * *", "UTC"))
			.validate(Utc::now())
			.is_ok()
	);
	for invalid in [long, late, agentless, negative, nul] {
		assert!(matches!(
			invalid.validate(Utc::now()),
			Err(Error::Invalid(_))
		));
	}
}

#[rstest]
fn a_definition_defaults_to_the_requesters_mode_and_three_failures() {
	// Act
	let parsed: Definition = serde_json::from_value(serde_json::json!({
		"title": "t", "mission": "r", "agent": {"id": "a", "version": "1.0.0"},
		"trigger": {"kind": "interval", "every_seconds": 3600}
	}))
	.unwrap();
	let untimed: Definition = serde_json::from_value(serde_json::json!({
		"title": "t", "mission": "r", "agent": {"id": "a", "version": "1.0.0"}
	}))
	.unwrap();
	// Assert
	assert_eq!(parsed.procedure, "");
	assert_eq!(parsed.approval_mode, None);
	assert_eq!(parsed.max_lateness_seconds, DEFAULT_MAX_LATENESS_SECONDS);
	assert_eq!(parsed.pause_after_failures, DEFAULT_PAUSE_AFTER_FAILURES);
	assert_eq!(untimed.trigger, None);
	assert!(untimed.validate(Utc::now()).is_ok());
}

#[rstest]
fn a_patch_sets_or_removes_the_time_trigger() {
	// Arrange
	let current = definition(cron("0 9 * * *", "UTC"));
	let removal: Patch =
		serde_json::from_value(serde_json::json!({"trigger": {"kind": "none"}})).unwrap();
	let hourly: Patch = serde_json::from_value(
		serde_json::json!({"trigger": {"kind": "interval", "every_seconds": 3600}}),
	)
	.unwrap();
	// Act
	let (removed, retimed) = removal.apply(&current);
	let (set, _) = hourly.apply(&current);
	// Assert
	assert_eq!(removed.trigger, None);
	assert!(retimed);
	assert_eq!(
		set.trigger,
		Some(Trigger::Interval {
			every_seconds: 3600
		})
	);
}

#[rstest]
#[case(49, None, true)]
#[case(1_000_000, None, true)]
#[case(11, Some(12), true)]
#[case(12, Some(12), false)]
fn only_a_plan_caps_how_many_duties_a_tenant_has(
	#[case] total: i64,
	#[case] tier: Option<u32>,
	#[case] admitted: bool,
) {
	// Act
	let result = admit_within_tier(total, tier);
	// Assert
	assert_eq!(result.is_ok(), admitted);
}

fn standing<'a>(halt: Option<Halt>, successor: Option<&'a str>) -> Standing<'a> {
	Standing {
		requester: "alice",
		halt,
		successor,
	}
}

fn person(subject: &str, administers: bool) -> Actor<'_> {
	Actor {
		subject,
		administers,
	}
}

#[rstest]
fn only_the_requester_edits_and_resumes_their_duty() {
	// Act + Assert
	assert!(permit(person("alice", false), standing(None, None), Change::Edit).is_ok());
	assert!(permit(person("admin", true), standing(None, None), Change::Edit).is_err());
	assert!(permit(person("bob", false), standing(None, None), Change::Pause).is_err());
	assert!(permit(person("admin", true), standing(None, None), Change::Pause).is_ok());
	assert!(permit(person("admin", true), standing(None, None), Change::Delete).is_ok());
	assert!(
		permit(
			person("alice", false),
			standing(Some(Halt::Failures), None),
			Change::Resume
		)
		.is_ok()
	);
	assert!(
		permit(
			person("admin", true),
			standing(Some(Halt::Paused), None),
			Change::Resume
		)
		.is_err()
	);
	assert!(matches!(
		permit(person("alice", false), standing(None, None), Change::Resume),
		Err(Error::Conflict(_))
	));
}

#[rstest]
fn a_duty_whose_requester_left_is_taken_over_only_by_its_named_successor() {
	// Arrange
	let waiting = standing(Some(Halt::RequesterLeft), Some("carol"));
	// Act + Assert
	assert!(
		permit(
			person("admin", true),
			standing(Some(Halt::RequesterLeft), None),
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
			standing(Some(Halt::RequesterLeft), None),
			Change::NameSuccessor
		)
		.is_err()
	);
	assert!(permit(person("carol", false), waiting, Change::TakeOver).is_ok());
	assert!(permit(person("admin", true), waiting, Change::TakeOver).is_err());
	assert!(permit(person("alice", false), waiting, Change::TakeOver).is_err());
	assert!(
		permit(
			person("carol", false),
			standing(Some(Halt::Paused), Some("carol")),
			Change::TakeOver
		)
		.is_err()
	);
	// Resuming never takes a Duty over (ADR 0145).
	assert!(permit(person("carol", false), waiting, Change::Resume).is_err());
	assert!(permit(person("alice", false), waiting, Change::Resume).is_err());
	assert!(permit(person("alice", false), waiting, Change::Edit).is_err());
}

/// A Run a Duty started may set up Duties too (#1012); a federated Run and
/// a chain without a person at its root may not.
#[rstest]
#[case(Caller { root_person: Some("alice"), federated: false }, Ok("alice"))]
#[case(Caller { root_person: Some("alice"), federated: true }, Err(()))]
#[case(Caller { root_person: None, federated: false }, Err(()))]
fn agents_set_up_duties_for_the_person_at_their_root(
	#[case] caller: Caller<'static>,
	#[case] expected: std::result::Result<&str, ()>,
) {
	// Act
	let result = agent_requester(caller).map_err(|_| ());
	// Assert
	assert_eq!(result, expected);
}

#[rstest]
fn failed_runs_in_a_row_pause_at_the_chosen_count_and_a_completion_resets() {
	// Act + Assert
	assert_eq!(after_settled(0, TickStatus::Failed, 3), (1, false));
	assert_eq!(after_settled(2, TickStatus::Failed, 3), (3, true));
	assert_eq!(after_settled(2, TickStatus::Cancelled, 3), (2, false));
	assert_eq!(after_settled(2, TickStatus::Completed, 3), (0, false));
	assert_eq!(after_settled(0, TickStatus::Failed, 1), (1, true));
	// 0 never pauses.
	assert_eq!(after_settled(99, TickStatus::Failed, 0), (100, false));
}

#[rstest]
fn halts_and_tick_statuses_round_trip() {
	for halt in [
		Halt::Paused,
		Halt::Failures,
		Halt::RequesterLeft,
		Halt::AgentVersionUnusable,
		Halt::ThreadDeleted,
		Halt::TenantDeleted,
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
	assert!(Halt::RequesterLeft.notifies());
}

#[rstest]
#[case(crate::TaskStatus::Running, None)]
#[case(crate::TaskStatus::Blocked, None)]
#[case(crate::TaskStatus::Failed, Some(TickStatus::Failed))]
#[case(crate::TaskStatus::Abandoned, Some(TickStatus::Cancelled))]
fn a_tick_ends_only_when_its_task_is_terminal(
	#[case] task: crate::TaskStatus,
	#[case] tick: Option<TickStatus>,
) {
	// Act + Assert: a blocked Task may run again, so its tick stays open.
	assert_eq!(TickStatus::of_task(task), tick);
	assert_eq!(tick.is_some(), task.is_terminal());
}

#[rstest]
fn a_definition_checks_its_router_and_watcher() {
	let mut definition = definition(Trigger::Interval {
		every_seconds: 3600,
	});
	definition.router = Some(scripts::Script {
		language: scripts::Language::Python,
		source: String::new(),
	});
	assert!(definition.validate(at("2026-11-19T00:00:00Z")).is_err());
	definition.router = None;
	definition.watcher = Some(scripts::Watcher {
		script: scripts::Script {
			language: scripts::Language::Shell,
			source: "echo '[]'".into(),
		},
		every_seconds: 30,
	});
	assert!(definition.validate(at("2026-11-19T00:00:00Z")).is_err());
}

#[rstest]
fn a_patch_sets_and_removes_scripts() {
	let before = definition(Trigger::Interval {
		every_seconds: 3600,
	});
	let router = scripts::Script {
		language: scripts::Language::Python,
		source: "print('{\"action\":\"drop\"}')".into(),
	};
	let patch: Patch = serde_json::from_value(serde_json::json!({
		"router": {"language": "python", "source": router.source},
		"watcher": {"script": {"language": "shell", "source": "echo '[]'"}, "every_seconds": 300},
	}))
	.unwrap();
	let (after, retimed) = patch.apply(&before);
	assert_eq!(after.router, Some(router));
	assert_eq!(
		after.watcher.as_ref().map(|watcher| watcher.every_seconds),
		Some(300)
	);
	assert!(!retimed, "scripts do not move the time trigger");
	let removal: Patch = serde_json::from_value(serde_json::json!({
		"router": {"kind": "none"},
		"watcher": {"kind": "none"},
	}))
	.unwrap();
	let (cleared, _) = removal.apply(&after);
	assert_eq!((cleared.router, cleared.watcher), (None, None));
}

/// Deleting a Duty removes its Budgets (#1012 decision 25): an Agent never
/// may when one targets it, and a person needs `budget.manage` unless they
/// set every one themselves.
#[rstest]
#[case(Deleter::Agent, &[], Some(BudgetRemoval::Free))]
#[case(Deleter::Agent, &["alice"], None)]
#[case(Deleter::Person, &[], Some(BudgetRemoval::Free))]
#[case(Deleter::Person, &["alice", "alice"], Some(BudgetRemoval::Free))]
#[case(Deleter::Person, &["alice", "admin"], Some(BudgetRemoval::NeedsBudgetManage))]
fn deleting_a_duty_never_lifts_a_budget_someone_else_set(
	#[case] deleter: Deleter,
	#[case] setters: &[&str],
	#[case] expected: Option<BudgetRemoval>,
) {
	let setters: Vec<String> = setters.iter().map(|setter| (*setter).to_owned()).collect();
	let judged = budget_removal(deleter, "alice", &setters);
	match expected {
		Some(removal) => assert_eq!(judged.unwrap(), removal),
		None => assert!(
			matches!(&judged, Err(Error::Invalid(message)) if message.contains("pause the Duty")),
			"{judged:?}"
		),
	}
}
