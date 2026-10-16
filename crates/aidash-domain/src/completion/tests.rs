use super::*;
use chrono::TimeZone;
use rstest::rstest;
use serde_json::json;

fn policy(value: serde_json::Value) -> Result<CompletionPolicy> {
	let policy: CompletionPolicy = serde_json::from_value(value)?;
	policy.validate()?;
	Ok(policy)
}

fn tests_policy(when: &str) -> CompletionPolicy {
	policy(json!({
		"version": "checks/1",
		"checks": [{"name": "tests", "capability": "shell", "command": "cargo test", "when": when}],
		"repair_allowance": 2,
		"verification_seconds": 600
	}))
	.unwrap()
}

fn file(scope: FileScope, path: &str, digest: &str) -> MountedFile {
	MountedFile {
		file_id: Uuid::nil(),
		path: path.into(),
		digest: digest.into(),
		size: 1,
		media_type: "text/plain".into(),
		scope,
		provenance: json!({}),
	}
}

fn at(seconds: i64) -> DateTime<Utc> {
	Utc.timestamp_opt(1_800_000_000 + seconds, 0).unwrap()
}

fn evidence(subject: &CheckSubject, outcome: CheckOutcome) -> VerificationEvidence {
	VerificationEvidence {
		subject: subject.clone(),
		operation_id: Some(Uuid::now_v7()),
		attempt: 1,
		outcome,
		exit_code: Some(if outcome == CheckOutcome::Passed {
			0
		} else {
			1
		}),
		diagnostics: "test result".into(),
		seconds: 10,
		finished_at: at(10),
	}
}

fn start(decision: Decision<'_>) -> CheckSubject {
	match decision {
		Decision::Start { subject, .. } => subject,
		other => panic!("expected a Check to start, got {other:?}"),
	}
}

const CAPABILITY: &str = "sha256:shell-binding";

#[rstest]
fn policy_defaults_and_bounds() {
	let parsed = policy(json!({
		"version": "checks/1",
		"checks": [{"name": "tests", "capability": "shell", "command": "make test"}]
	}))
	.unwrap();
	assert_eq!(parsed.repair_allowance(), 3);
	assert_eq!(parsed.verification_seconds(), 1800);
	assert!(!parsed.exception_permitted());
	assert_eq!(parsed.checks()[0].when, CheckWhen::ContentChanged);
	assert!(parsed.needs_start_content());
	// Defaults are omitted, so a definition keeps its digest when written back.
	assert_eq!(
		serde_json::to_value(&parsed).unwrap(),
		json!({"version":"checks/1","checks":[{"name":"tests","capability":"shell","command":"make test"}],"repair_allowance":3,"verification_seconds":1800})
	);
	let check = |extra: serde_json::Value| {
		let mut value = json!({"name": "tests", "capability": "shell", "command": "make test"});
		value
			.as_object_mut()
			.unwrap()
			.extend(extra.as_object().unwrap().clone());
		json!({"version": "checks/1", "checks": [value]})
	};
	for invalid in [
		json!({"version": "checks/1", "checks": []}),
		json!({"version": "checks/2", "checks": [{"name": "t", "capability": "shell", "command": "x"}]}),
		json!({"version": "checks/1", "checks": [{"name": "t", "capability": "shell", "command": "x"}], "repair_allowance": 17}),
		json!({"version": "checks/1", "checks": [{"name": "t", "capability": "shell", "command": "x"}], "verification_seconds": 0}),
		json!({"version": "checks/1", "checks": [{"name": "t", "capability": "shell", "command": "x"}, {"name": "t", "capability": "shell", "command": "y"}]}),
		check(json!({"capability": "code_interpreter"})),
		check(json!({"name": "Tests"})),
		check(json!({"name": "-tests"})),
		check(json!({"command": "  "})),
		check(json!({"command": "a\u{0}b"})),
		check(json!({"timeout_seconds": 0})),
		check(json!({"timeout_seconds": 601})),
		check(json!({"arguments": "chosen by the model"})),
	] {
		assert!(policy(invalid.clone()).is_err(), "{invalid}");
	}
}

#[rstest]
fn content_digest_covers_every_scope_and_ignores_order() {
	let a = file(FileScope::Working, "src/lib.rs", "aa");
	let b = file(FileScope::References, "spec.md", "bb");
	assert_eq!(
		content_digest(&[a.clone(), b.clone()]),
		content_digest(&[b.clone(), a.clone()])
	);
	assert_ne!(
		content_digest(std::slice::from_ref(&a)),
		content_digest(&[file(FileScope::Working, "src/lib.rs", "ab")])
	);
	assert_ne!(
		content_digest(std::slice::from_ref(&a)),
		content_digest(&[file(FileScope::Received, "src/lib.rs", "aa")])
	);
	assert_ne!(content_digest(&[a, b]), empty_content());
}

#[rstest]
fn unchanged_content_needs_no_verification() {
	let policy = tests_policy("content_changed");
	let progress = CompletionProgress {
		start_content: Some("sha256:start".into()),
		..Default::default()
	};
	let candidate = Candidate {
		content: Some("sha256:start"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	assert_eq!(
		decide(&policy, &progress, &candidate),
		Decision::Complete(CompletionBasis::NotRequired)
	);
	// Without a Working Area nothing changed either.
	let progress = CompletionProgress {
		start_content: Some(empty_content()),
		..Default::default()
	};
	let candidate = Candidate {
		content: None,
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	assert_eq!(
		decide(&policy, &progress, &candidate),
		Decision::Complete(CompletionBasis::NotRequired)
	);
}

#[rstest]
fn changed_content_completes_only_with_passing_evidence_for_it() {
	let policy = tests_policy("content_changed");
	let mut progress = CompletionProgress {
		start_content: Some("sha256:start".into()),
		..Default::default()
	};
	let candidate = Candidate {
		content: Some("sha256:edited"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	let subject = start(decide(&policy, &progress, &candidate));
	assert_eq!(subject.content_digest, "sha256:edited");
	assert_eq!(subject.capability_digest, CAPABILITY);
	progress.record(evidence(&subject, CheckOutcome::Passed));
	assert_eq!(
		decide(&policy, &progress, &candidate),
		Decision::Complete(CompletionBasis::Verified)
	);
	// A later edit invalidates the evidence: the Check runs again.
	let edited_again = Candidate {
		content: Some("sha256:edited-again"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	assert_eq!(
		start(decide(&policy, &progress, &edited_again)).content_digest,
		"sha256:edited-again"
	);
}

#[rstest]
fn evidence_of_another_subject_never_counts() {
	let policy = tests_policy("always");
	let progress = CompletionProgress::default();
	let candidate = Candidate {
		content: Some("sha256:content"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	let subject = start(decide(&policy, &progress, &candidate));
	for foreign in [
		CheckSubject {
			content_digest: "sha256:pre-change".into(),
			..subject.clone()
		},
		CheckSubject {
			check_digest: tests_policy("content_changed").checks()[0].digest(),
			..subject.clone()
		},
		CheckSubject {
			capability_digest: "sha256:other-shell-version".into(),
			..subject.clone()
		},
		CheckSubject {
			check: "lint".into(),
			..subject.clone()
		},
	] {
		let mut progress = progress.clone();
		progress.record(evidence(&foreign, CheckOutcome::Passed));
		assert_eq!(start(decide(&policy, &progress, &candidate)), subject);
	}
}

#[rstest]
fn failures_are_repaired_until_the_allowance_is_spent() {
	let policy = tests_policy("always");
	let mut progress = CompletionProgress::default();
	let candidate = Candidate {
		content: Some("sha256:content"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	let subject = start(decide(&policy, &progress, &candidate));
	for outcome in [CheckOutcome::Failed, CheckOutcome::TimedOut] {
		let mut progress = progress.clone();
		progress.record(evidence(&subject, outcome));
		assert!(matches!(
			decide(&policy, &progress, &candidate),
			Decision::Repair(evidence) if evidence.outcome == outcome
		));
	}
	progress.record(evidence(&subject, CheckOutcome::Failed));
	progress.repairs = 2;
	assert_eq!(
		decide(&policy, &progress, &candidate),
		Decision::Review(ReviewReason::RepairExhausted {
			check: "tests".into()
		})
	);
	// An accepted human input renews the allowance.
	progress.renew(4);
	assert_eq!(progress.repairs, 0);
	assert!(matches!(
		decide(&policy, &progress, &candidate),
		Decision::Repair(_)
	));
	progress.renew(4);
	progress.repairs = 1;
	progress.renew(3);
	assert_eq!(progress.repairs, 1);
}

/// A repair consumes the failure it delivered: the next candidate runs the
/// Check again, even when the model left the files unchanged.
#[rstest]
fn a_repair_consumes_the_failure_it_delivered() {
	let policy = tests_policy("always");
	let candidate = Candidate {
		content: Some("sha256:content"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	let subject = start(decide(&policy, &CompletionProgress::default(), &candidate));
	let mut progress = CompletionProgress::default();
	progress.record(evidence(&subject, CheckOutcome::Failed));
	progress.repair(&subject);
	assert_eq!(progress.repairs, 1);
	assert!(progress.evidence.is_empty());
	assert_eq!(start(decide(&policy, &progress, &candidate)), subject);
}

#[rstest]
fn refusals_that_need_a_person_pause_for_review() {
	let policy = tests_policy("always");
	let candidate = Candidate {
		content: Some("sha256:content"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	let subject = start(decide(&policy, &CompletionProgress::default(), &candidate));
	for (outcome, expected) in [
		(
			CheckOutcome::Unavailable,
			ReviewReason::Unavailable {
				check: "tests".into(),
				detail: "test result".into(),
			},
		),
		(
			CheckOutcome::Denied,
			ReviewReason::Denied {
				check: "tests".into(),
				detail: "test result".into(),
			},
		),
	] {
		let mut progress = CompletionProgress::default();
		progress.record(evidence(&subject, outcome));
		assert_eq!(
			decide(&policy, &progress, &candidate),
			Decision::Review(expected)
		);
		// A retry forgets the refusal, so the Check runs again.
		progress.retry();
		assert_eq!(start(decide(&policy, &progress, &candidate)), subject);
	}
	let no_area = Candidate {
		content: None,
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	assert!(matches!(
		decide(&policy, &CompletionProgress::default(), &no_area),
		Decision::Review(ReviewReason::Unavailable { .. })
	));
	let unbound = Candidate {
		content: Some("sha256:content"),
		capability: None,
		unavailable: None,
	};
	assert_eq!(
		decide(&policy, &CompletionProgress::default(), &unbound),
		Decision::Review(ReviewReason::Unbound {
			check: "tests".into(),
			capability: "shell".into(),
		})
	);
	// Without the capability there may be no Working Area either; the
	// Binding defect, which no retry recovers, is what the review names.
	let neither = Candidate {
		content: None,
		..unbound
	};
	assert!(matches!(
		decide(&policy, &CompletionProgress::default(), &neither),
		Decision::Review(ReviewReason::Unbound { .. })
	));
	let spent = CompletionProgress {
		seconds: 600,
		..Default::default()
	};
	assert_eq!(
		decide(&policy, &spent, &candidate),
		Decision::Review(ReviewReason::TimeExhausted)
	);
	// A failure that spent the last of the time is not sent for repair: the
	// repaired candidate could never run its Check.
	let mut last = spent;
	last.record(evidence(&subject, CheckOutcome::Failed));
	assert_eq!(
		decide(&policy, &last, &candidate),
		Decision::Review(ReviewReason::TimeExhausted)
	);
}

#[rstest]
#[case::own(Some(45), Some(30), 45)]
#[case::binding_ceiling(None, Some(30), 30)]
#[case::node_default(None, None, 120)]
fn an_omitted_check_timeout_is_what_an_omitted_shell_argument_gets(
	#[case] own: Option<u64>,
	#[case] ceiling: Option<u64>,
	#[case] expected: u64,
) {
	assert_eq!(check_timeout(own, ceiling, 120), expected);
}

#[rstest]
fn checks_apply_in_order_and_all_must_pass() {
	let policy = policy(json!({
		"version": "checks/1",
		"checks": [
			{"name": "build", "capability": "shell", "command": "make", "when": "always"},
			{"name": "tests", "capability": "shell", "command": "make test", "when": "always"}
		]
	}))
	.unwrap();
	let candidate = Candidate {
		content: Some("sha256:content"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	let mut progress = CompletionProgress::default();
	let build = start(decide(&policy, &progress, &candidate));
	assert_eq!(build.check, "build");
	progress.record(evidence(&build, CheckOutcome::Passed));
	let tests = start(decide(&policy, &progress, &candidate));
	assert_eq!(tests.check, "tests");
	progress.record(evidence(&tests, CheckOutcome::Passed));
	assert_eq!(
		decide(&policy, &progress, &candidate),
		Decision::Complete(CompletionBasis::Verified)
	);
}

#[rstest]
fn pending_checks_time_out_and_keys_are_stable() {
	let subject = CheckSubject {
		check: "tests".into(),
		check_digest: "sha256:check".into(),
		capability_digest: CAPABILITY.into(),
		content_digest: "sha256:content".into(),
	};
	let pending = PendingCheck {
		subject: subject.clone(),
		operation_id: Uuid::now_v7(),
		attempt: 1,
		timeout_seconds: 60,
		started_at: at(0),
		running_since: None,
	};
	assert_eq!(
		pending
			.clone()
			.finish(true, Some(0), "ok", None, None, at(5))
			.outcome,
		CheckOutcome::Passed
	);
	assert_eq!(
		pending
			.clone()
			.finish(false, Some(1), "failed", None, None, at(5))
			.outcome,
		CheckOutcome::Failed
	);
	assert_eq!(
		pending
			.clone()
			.finish(false, None, "", None, None, at(60))
			.outcome,
		CheckOutcome::TimedOut
	);
	// A Check that has not started yet also gets its start allowance; once
	// seen running, its timeout counts from then.
	assert!(!pending.overdue(at(780)));
	assert!(pending.overdue(at(781)));
	let running = PendingCheck {
		running_since: Some(at(500)),
		..pending.clone()
	};
	assert!(!running.overdue(at(680)));
	assert!(running.overdue(at(681)));
	let long = "é".repeat(MAX_DIAGNOSTIC_BYTES);
	let recorded = pending
		.clone()
		.finish(false, Some(1), &long, None, None, at(5))
		.diagnostics;
	assert!(recorded.len() <= MAX_DIAGNOSTIC_BYTES);
	assert!(long.ends_with(&recorded));
	// The runner's runtime decides, not when the harness harvested the result.
	let quick = PendingCheck {
		timeout_seconds: 3,
		..pending.clone()
	}
	.finish(false, Some(1), "failed", Some(1000), None, at(5));
	assert_eq!((quick.outcome, quick.seconds), (CheckOutcome::Failed, 1));
	// Classification uses the exact runtime; only the charge is rounded up.
	let short = PendingCheck {
		timeout_seconds: 1,
		..pending.clone()
	};
	let early = short
		.clone()
		.finish(false, Some(1), "failed", Some(200), None, at(5));
	assert_eq!((early.outcome, early.seconds), (CheckOutcome::Failed, 1));
	let expired = short
		.clone()
		.finish(false, None, "", Some(1000), None, at(5));
	assert_eq!(
		(expired.outcome, expired.seconds),
		(CheckOutcome::TimedOut, 1)
	);
	let killed = pending
		.clone()
		.finish(false, None, "", Some(60_000), None, at(900));
	assert_eq!(
		(killed.outcome, killed.seconds),
		(CheckOutcome::TimedOut, 60)
	);
	// An unmeasured attempt never spends more than its timeout: a worker that
	// was down for an hour does not charge the hour as Check time.
	let lost = pending.clone().unavailable("runner lost it", at(3600));
	assert_eq!(
		(lost.outcome, lost.seconds),
		(CheckOutcome::Unavailable, 60)
	);
	let unreported = pending
		.clone()
		.finish(false, None, "", None, None, at(3600));
	assert_eq!(
		(unreported.outcome, unreported.seconds),
		(CheckOutcome::TimedOut, 60)
	);
	// A started second is charged whole, so a sub-second attempt still
	// spends verification time.
	let instant = pending.finish(
		false,
		Some(1),
		"failed",
		None,
		None,
		at(0) + chrono::Duration::milliseconds(300),
	);
	assert_eq!(
		(instant.outcome, instant.seconds),
		(CheckOutcome::Failed, 1)
	);
	// The runner's word on its deadline decides over the measured runtime,
	// which is only a conservative bound for charging.
	let bounded = short
		.clone()
		.finish(false, Some(1), "failed", Some(1100), Some(false), at(5));
	assert_eq!(
		(bounded.outcome, bounded.seconds),
		(CheckOutcome::Failed, 2)
	);
	let late_zero = short
		.clone()
		.finish(true, Some(0), "ok", Some(1000), Some(true), at(5));
	assert_eq!(late_zero.outcome, CheckOutcome::TimedOut);
	let deadline = short.finish(false, None, "", Some(900), Some(true), at(5));
	assert_eq!(
		(deadline.outcome, deadline.seconds),
		(CheckOutcome::TimedOut, 1)
	);

	let run = Uuid::now_v7();
	assert_eq!(subject.key(run, 1), subject.key(run, 1));
	assert_ne!(subject.key(run, 1), subject.key(run, 2));
	assert_ne!(subject.key(run, 1), subject.key(Uuid::now_v7(), 1));
}

#[rstest]
fn progress_keeps_the_newest_evidence_per_subject() {
	let mut progress = CompletionProgress::default();
	assert!(progress.is_empty());
	let subject = |content: usize| CheckSubject {
		check: "tests".into(),
		check_digest: "sha256:check".into(),
		capability_digest: CAPABILITY.into(),
		content_digest: format!("sha256:{content}"),
	};
	progress.record(evidence(&subject(0), CheckOutcome::Failed));
	progress.record(evidence(&subject(0), CheckOutcome::Passed));
	assert_eq!(progress.evidence.len(), 1);
	assert_eq!(
		progress.latest(&subject(0)).unwrap().outcome,
		CheckOutcome::Passed
	);
	assert_eq!(progress.seconds, 20);
	for content in 1..=40 {
		progress.record(evidence(&subject(content), CheckOutcome::Passed));
	}
	assert_eq!(progress.evidence.len(), MAX_EVIDENCE);
	assert!(progress.latest(&subject(0)).is_none());
	assert!(progress.latest(&subject(40)).is_some());
}

#[rstest]
fn completion_basis_round_trips() {
	for basis in [
		CompletionBasis::Verified,
		CompletionBasis::NotRequired,
		CompletionBasis::Excepted,
	] {
		assert_eq!(CompletionBasis::parse(basis.as_str()).unwrap(), basis);
		assert_eq!(serde_json::to_value(basis).unwrap(), json!(basis.as_str()));
	}
	assert!(CompletionBasis::parse("passed").is_err());
}

/// An unavailable Working Area goes to a Completion Review, even when passing
/// evidence for its last manifest exists.
#[test]
fn an_unavailable_area_never_reuses_evidence() {
	// Arrange
	let policy = tests_policy("always");
	let mut progress = CompletionProgress::default();
	let candidate = Candidate {
		content: Some("sha256:edited"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	let subject = start(decide(&policy, &progress, &candidate));
	progress.record(evidence(&subject, CheckOutcome::Passed));
	assert_eq!(
		decide(&policy, &progress, &candidate),
		Decision::Complete(CompletionBasis::Verified)
	);
	// Act
	let decision = decide(
		&policy,
		&progress,
		&Candidate {
			unavailable: Some("an earlier operation's effect is uncertain"),
			..candidate
		},
	);
	// Assert
	assert_eq!(
		decision,
		Decision::Review(ReviewReason::Unavailable {
			check: "tests".into(),
			detail: "an earlier operation's effect is uncertain".into(),
		})
	);
}

/// An unavailable Area's manifest cannot prove its files unchanged: an
/// uncertain effect may not have been published. `content_changed` Checks do
/// not skip it as `not_required`.
#[test]
fn an_unavailable_area_is_never_unchanged_content() {
	// Arrange
	let policy = tests_policy("content_changed");
	let progress = CompletionProgress {
		start_content: Some("sha256:start".into()),
		..CompletionProgress::default()
	};
	let candidate = Candidate {
		content: Some("sha256:start"),
		capability: Some(CAPABILITY),
		unavailable: None,
	};
	assert_eq!(
		decide(&policy, &progress, &candidate),
		Decision::Complete(CompletionBasis::NotRequired)
	);
	// Act
	let decision = decide(
		&policy,
		&progress,
		&Candidate {
			unavailable: Some("the Working Area is uncertain"),
			..candidate
		},
	);
	// Assert
	assert_eq!(
		decision,
		Decision::Review(ReviewReason::Unavailable {
			check: "tests".into(),
			detail: "the Working Area is uncertain".into(),
		})
	);
}

/// Neither retrying nor new input changes the Run's pinned Bindings, so an
/// unbound capability's review offers neither. New input cannot lift a
/// refusal either, so a refused Check's review does not offer it.
#[test]
fn reviews_offer_only_actions_that_can_recover() {
	let unbound = ReviewReason::Unbound {
		check: "tests".into(),
		capability: "shell".into(),
	}
	.prompt();
	assert!(
		!unbound.contains("Answer retry")
			&& !unbound.contains("send a new message")
			&& unbound.contains("start a new one"),
		"{unbound}"
	);
	let denied = ReviewReason::Denied {
		check: "tests".into(),
		detail: "exceeds Binding".into(),
	}
	.prompt();
	assert!(
		!denied.contains("send a new message") && denied.contains("start a new Run"),
		"{denied}"
	);
}

/// Neither retrying nor new input restores spent verification time, so that
/// review offers neither.
#[test]
fn a_time_exhausted_review_does_not_offer_retry() {
	let time = ReviewReason::TimeExhausted.prompt();
	assert!(
		!time.contains("retry") && !time.contains("new message"),
		"{time}"
	);
	// New input renews the Repair Allowance, not the Run's steps.
	let steps = ReviewReason::StepsExhausted {
		check: "tests".into(),
	}
	.prompt();
	assert!(steps.contains("no step left") && !steps.contains("Repair Allowance"));
	assert!(!steps.contains("new message"));
	assert!(
		ReviewReason::RepairExhausted {
			check: "tests".into()
		}
		.prompt()
		.contains("retry")
	);
}
