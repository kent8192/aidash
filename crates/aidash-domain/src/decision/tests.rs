use super::*;
use chrono::{TimeZone, Utc};
fn config() -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Compaction,
		answer_type: AnswerType::Noul,
		description: "Retain required history".into(),
		provider_contract: PROVIDER.into(),
		provider_credential: "openrouter".into(),
		model: "typesafe/jev-1.13".into(),
		builder: BUILDER.into(),
		option_source: OPTION_SOURCE.into(),
		rule: RULE.into(),
		keep_threshold: Probability::half(),
		mode: Mode::Enforce,
		max_calls_per_run: 20,
		state_retention: policy::StateRetention::default(),
		state_budget: StateBudget::default(),
	}
}
#[test]
fn declarations_reject_unsupported_contracts_mutable_models_and_legacy_limits() {
	let original = serde_json::to_value(config()).unwrap();
	for (field, value) in [
		("hook", "approval"),
		("answer_type", "choice"),
		("provider_contract", "unknown/1"),
		("builder", "custom-code/1"),
		("option_source", "live/1"),
		("rule", "unknown/2"),
		("model", "jev-latest"),
		("model", "jev-1.13"),
		("model", "~typesafe/jev-latest"),
		("model", "typesafe/jev-latest"),
		("model", "typesafe/jev-1"),
		("model", "typesafe/jev-1.13.0"),
		("model", "typesafe/jev-01.13"),
		("model", "typesafe/jev-1.13-20260917"),
		("model", "other/jev-1.13"),
		("provider_credential", "typesafe"),
		("provider_credential", ""),
		("description", " "),
	] {
		let mut json = original.clone();
		json[field] = value.into();
		assert!(
			serde_json::from_value::<DeciderConfig>(json).map_or(true, |c| c.validate().is_err()),
			"{field}={value}"
		);
	}
	// A Capability Token carries at most 256 bytes of model ID.
	let mut json = original.clone();
	json["model"] = format!("typesafe/jev-1.{}", "1".repeat(256)).into();
	assert!(serde_json::from_value::<DeciderConfig>(json).map_or(true, |c| c.validate().is_err()));
	for field in [
		"max_request_bytes",
		"max_questions",
		"max_response_bytes",
		"compactor",
		// Transport and keys come only from the Provider Catalog and Tenant binding.
		"endpoint",
		"credential_env",
	] {
		let mut json = original.clone();
		json[field] = 1.into();
		assert!(serde_json::from_value::<DeciderConfig>(json).is_err());
	}
	config().validate().unwrap();
}
#[test]
fn served_models_match_their_pin_or_a_dated_snapshot_of_it() {
	assert!(served_by("typesafe/jev-1.13", "typesafe/jev-1.13"));
	assert!(served_by("typesafe/jev-1.13", "typesafe/jev-1.13-20260917"));
	for served in [
		"typesafe/jev-1.14",
		"typesafe/jev-1.13-latest",
		"typesafe/jev-1.13-2026091",
		"typesafe/jev-1.130",
		"typesafe/jev-1.13-20260917-x",
	] {
		assert!(!served_by("typesafe/jev-1.13", served), "{served}");
	}
}
#[test]
fn binary64_evidence_round_trips_subnormals_negative_zero_and_boundary_neighbors() {
	for bits in [
		0,
		1,
		0x8000000000000000,
		0x3fdfffffffffffff,
		0x3fe0000000000000,
		0x3fe0000000000001,
		0x3ff0000000000000,
	] {
		let p = Probability::new(f64::from_bits(bits)).unwrap();
		let json = serde_json::to_string(&p).unwrap();
		let decoded: Probability = serde_json::from_str(&json).unwrap();
		assert_eq!(decoded.value().to_bits(), bits);
	}
	for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01, 1.01] {
		assert!(Probability::new(v).is_err());
	}
	for encoded in [
		"7ff8000000000000",
		"7ff0000000000000",
		"3ff0000000000001",
		"-0.5",
		"3FE0000000000000",
	] {
		assert!(serde_json::from_value::<Probability>(serde_json::json!(encoded)).is_err());
	}
}
#[test]
fn keep_threshold_is_inclusive_and_result_precedes_call() {
	let below =
		Probability::new(f64::from_bits(Probability::half().value().to_bits() - 1)).unwrap();
	let half = Probability::half();
	assert_eq!(branch(below, half, half), Branch::Keep);
	assert_eq!(branch(half, below, half), Branch::TruncateResult);
	assert_eq!(branch(below, below, half), Branch::Drop);
	assert_eq!(branch(half, half, half), Branch::Keep);
}
#[test]
fn narrowing_cannot_increase_deletion_and_intersections_preserve_both_constraints() {
	let c = config();
	let a = Restrictions {
		keep_threshold: Probability::new(0.25).unwrap(),
		preserve_recent: 8,
		forbid_apply: false,
		max_calls_per_run: Some(10),
	};
	let b = Restrictions {
		keep_threshold: Probability::new(0.1).unwrap(),
		preserve_recent: 6,
		forbid_apply: true,
		max_calls_per_run: None,
	};
	let intersection = a.intersect(&b, &c).unwrap();
	assert_eq!(intersection.keep_threshold, b.keep_threshold);
	assert_eq!(intersection.preserve_recent, 8);
	assert!(intersection.forbid_apply);
	assert_eq!(intersection.max_calls_per_run, Some(10));
	assert!(intersection.at_least_as_strict(&a) && intersection.at_least_as_strict(&b));
	assert!(!b.at_least_as_strict(&a));
	// A Binding may lower the call limit, never raise it or spend nothing.
	for limit in [21, 0] {
		assert!(
			Restrictions {
				max_calls_per_run: Some(limit),
				..a.clone()
			}
			.validate(&c)
			.is_err(),
			"{limit}"
		);
	}
	assert!(
		Restrictions {
			keep_threshold: Probability::new(0.6).unwrap(),
			..a.clone()
		}
		.validate(&c)
		.is_err()
	);
	assert!(
		Restrictions {
			preserve_recent: 5,
			..a
		}
		.validate(&c)
		.is_err()
	);
}
#[test]
fn full_state_is_opt_in_and_retention_intersects_source_deadlines() {
	let now = Utc.with_ymd_and_hms(2026, 10, 8, 0, 0, 0).unwrap();
	assert_eq!(
		policy::StateRetention::default()
			.expires_at(now, None)
			.unwrap(),
		None
	);
	let enabled = policy::StateRetention {
		enabled: true,
		..Default::default()
	};
	assert_eq!(
		enabled.expires_at(now, None).unwrap(),
		Some(now + chrono::Duration::days(7))
	);
	let source = now + chrono::Duration::days(2);
	assert_eq!(enabled.expires_at(now, Some(source)).unwrap(), Some(source));
	assert!(enabled.expires_at(now, Some(now)).is_err());
	assert!(
		policy::StateRetention {
			enabled: true,
			days: 31
		}
		.expires_at(now, None)
		.is_err()
	);
}
#[test]
fn explicit_allowances_do_not_reset_after_policy_widening_or_recovery() {
	let pin = DeciderPin {
		identity: QualifiedRef::builtin("aidash://node", "decision"),
		definition_digest: format!("sha256:{}", "a".repeat(64)),
		configuration_digest: format!("sha256:{}", "b".repeat(64)),
		provider_implementation: "node-decision-adapter:23".into(),
	};
	let pinned = policy::ExecutionAllowance {
		decider: pin.clone(),
		max_calls_per_run: 3,
	};
	let mut current = policy::ExecutionAllowance {
		decider: pin,
		max_calls_per_run: 50,
	};
	assert_eq!(pinned.remaining(&current, 2).unwrap(), 1);
	assert_eq!(pinned.remaining(&current, 4).unwrap(), 0);
	current.max_calls_per_run = 1;
	assert_eq!(pinned.remaining(&current, 2).unwrap(), 0);
	current.decider.provider_implementation = "node-decision-adapter:24".into();
	assert!(pinned.remaining(&current, 0).is_err());
	current.decider = pinned.decider.clone();
	current.decider.configuration_digest = format!("sha256:{}", "c".repeat(64));
	assert!(pinned.remaining(&current, 0).is_err());
	for implementation in ["", " \t "] {
		let mut corrupt = pinned.decider.clone();
		corrupt.provider_implementation = implementation.into();
		assert!(corrupt.validate().is_err());
	}
}
#[test]
fn answer_coverage_rejects_missing_extra_or_undescribed_questions() {
	let mut q = Questions::from([(
		"q".into(),
		Question {
			description: "Keep?".into(),
			answer_type: AnswerType::Noul,
		},
	)]);
	let a = BTreeMap::from([("q".into(), Probability::half())]);
	validate_answers(&q, &a).unwrap();
	assert!(validate_answers(&q, &BTreeMap::new()).is_err());
	let mut extra = a.clone();
	extra.insert("extra".into(), Probability::half());
	assert!(validate_answers(&q, &extra).is_err());
	q.get_mut("q").unwrap().description.clear();
	assert!(validate_answers(&q, &a).is_err());
}
#[test]
fn tool_descriptors_cannot_smuggle_decision_restrictions() {
	let narrow = crate::registry::bindings::Narrowing {
		decision: Some(Restrictions::default()),
		..Default::default()
	};
	assert!(crate::tool::providers::validate_restrictions("file_read", &narrow).is_err());
}
#[test]
fn state_budgets_stay_in_the_default_mode_unless_long_context_is_declared() {
	let budget = StateBudget::default();
	budget.validate().unwrap();
	assert_eq!(budget.request_limit(24_000), Some(32_000));
	assert_eq!(budget.request_limit(24_001), Some(600_000));
	assert_eq!(budget.request_limit(560_001), None);
	let default_only = StateBudget {
		long_context_request_bytes: None,
		long_context_state_bytes: None,
		..budget.clone()
	};
	default_only.validate().unwrap();
	assert_eq!(default_only.request_limit(24_001), None);
	for invalid in [
		StateBudget {
			default_request_bytes: 32_001,
			..budget.clone()
		},
		StateBudget {
			default_state_bytes: 32_000,
			..budget.clone()
		},
		StateBudget {
			long_context_state_bytes: None,
			..budget.clone()
		},
		// A long-context bound below the default one would under-charge.
		StateBudget {
			long_context_request_bytes: Some(25_001),
			long_context_state_bytes: Some(25_000),
			..budget.clone()
		},
		StateBudget {
			input_stages: vec![],
			..budget.clone()
		},
		StateBudget {
			input_stages: vec![Some(200), None],
			..budget.clone()
		},
		StateBudget {
			input_stages: vec![None, Some(60), Some(200)],
			..budget.clone()
		},
		StateBudget {
			input_stages: vec![None, Some(10)],
			..budget.clone()
		},
	] {
		assert!(invalid.validate().is_err(), "{invalid:?}");
	}
	let mut json = serde_json::to_value(config()).unwrap();
	json["max_calls_per_run"] = 0.into();
	assert!(
		serde_json::from_value::<DeciderConfig>(json)
			.unwrap()
			.validate()
			.is_err()
	);
}
#[test]
fn the_configuration_pin_commits_to_the_mode() {
	let enforce = config();
	let shadow = DeciderConfig {
		mode: Mode::Shadow,
		..config()
	};
	assert_eq!(
		enforce.parameters_digest().unwrap(),
		shadow.parameters_digest().unwrap()
	);
	assert_ne!(enforce.digest().unwrap(), shadow.digest().unwrap());
}
#[test]
fn the_admitted_allowance_takes_the_lower_call_limit() {
	let bound = |limit| BoundDecider {
		pin: DeciderPin {
			identity: QualifiedRef::builtin("aidash://node", "decision"),
			definition_digest: format!("sha256:{}", "a".repeat(64)),
			configuration_digest: format!("sha256:{}", "b".repeat(64)),
			provider_implementation: "node-decision-adapter:23".into(),
		},
		config: config(),
		restrictions: Restrictions {
			max_calls_per_run: limit,
			..Default::default()
		},
	};
	assert_eq!(bound(None).allowance().max_calls_per_run, 20);
	assert_eq!(bound(Some(4)).allowance().max_calls_per_run, 4);
}
mod pruning_rules {
	use super::*;
	use crate::context::{ContextEvent, HistoryEntry};
	use crate::decision::pruning::{Units, apply, protected};
	use crate::provider::ToolCall;
	use serde_json::json;

	fn assistant(seq: u64, text: &str, calls: &[&str]) -> HistoryEntry {
		HistoryEntry {
			seq,
			event: ContextEvent::Assistant {
				epoch: 1,
				text: text.into(),
				calls: calls.iter().map(|call| (*call).to_owned()).collect(),
			},
		}
	}
	fn tool(seq: u64, id: &str, result: &str) -> HistoryEntry {
		HistoryEntry {
			seq,
			event: ContextEvent::Tool {
				call: ToolCall {
					id: id.into(),
					name: "file_read".into(),
					arguments: json!({"path": id}),
				},
				result: json!(result),
			},
		}
	}

	#[test]
	fn legacy_protects_events_and_native_protects_whole_turns() {
		assert!(protected(0, None, 20, None, 6));
		assert!(!protected(13, None, 20, None, 6));
		assert!(protected(14, None, 20, None, 6));
		// Event 13 sits in the third of nine turns, far from the protected tail.
		assert!(!protected(13, Some(2), 20, Some(9), 6));
		assert!(protected(13, Some(3), 20, Some(9), 6));
		assert!(protected(1, Some(0), 20, Some(9), 6));
	}

	#[test]
	fn native_elides_dropped_results_and_removes_a_turn_only_as_a_whole() {
		let history = vec![
			assistant(1, "", &["a", "b"]),
			tool(2, "a", "first"),
			tool(3, "b", "second"),
			assistant(4, "I read c", &["c"]),
			tool(5, "c", "third"),
		];
		let units = Units::of(&history, true);
		assert_eq!(units.turns, Some(2));
		let whole = apply(
			&history,
			&BTreeMap::from([(1, Branch::Drop), (2, Branch::Drop), (4, Branch::Drop)]),
			&units,
		);
		// The first turn has no text, so it leaves whole; the second keeps its
		// response and elides the dropped result under its call.
		assert_eq!(whole.removed, 3);
		assert_eq!(whole.history.len(), 2);
		let ContextEvent::Tool { result, .. } = &whole.history[1].event else {
			panic!("tool expected");
		};
		assert!(ContextEvent::is_elided_result(result));
		let partial = apply(&history, &BTreeMap::from([(1, Branch::Drop)]), &units);
		assert_eq!(partial.removed, 0);
		assert_eq!(partial.history.len(), 5);
	}

	#[test]
	fn legacy_removes_dropped_events_and_truncates_long_results_only() {
		let history = vec![
			tool(1, "a", "short"),
			tool(2, "b", &"x".repeat(500)),
			tool(3, "c", "short"),
		];
		let units = Units::of(&history, false);
		let pruned = apply(
			&history,
			&BTreeMap::from([
				(0, Branch::TruncateResult),
				(1, Branch::TruncateResult),
				(2, Branch::Drop),
			]),
			&units,
		);
		assert_eq!((pruned.removed, pruned.truncated), (1, 1));
		assert_eq!(pruned.history.len(), 2);
		assert_eq!(pruned.history[0], history[0]);
	}
}
