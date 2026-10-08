use super::*;
use chrono::{TimeZone, Utc};
fn config() -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Compaction,
		answer_type: AnswerType::Noul,
		description: "Retain required history".into(),
		provider_contract: PROVIDER.into(),
		endpoint: "https://api.typesafe.ai/v1/systemone".into(),
		model: "jev-1.13.0".into(),
		credential_env: "AIDASH_SECRET_JEV".into(),
		builder: BUILDER.into(),
		option_source: OPTION_SOURCE.into(),
		rule: RULE.into(),
		keep_threshold: Probability::half(),
		mode: Mode::Enforce,
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
		("model", "jev-preview"),
		("model", "jev-1.13.0+alias"),
		("model", "jev-01.13.0"),
		("description", " "),
	] {
		let mut json = original.clone();
		json[field] = value.into();
		assert!(
			serde_json::from_value::<DeciderConfig>(json).map_or(true, |c| c.validate().is_err()),
			"{field}={value}"
		);
	}
	for field in [
		"max_request_bytes",
		"max_questions",
		"max_response_bytes",
		"compactor",
	] {
		let mut json = original.clone();
		json[field] = 1.into();
		assert!(serde_json::from_value::<DeciderConfig>(json).is_err());
	}
	config().validate().unwrap();
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
	};
	let b = Restrictions {
		keep_threshold: Probability::new(0.1).unwrap(),
		preserve_recent: 6,
		forbid_apply: true,
	};
	let intersection = a.intersect(&b, &c).unwrap();
	assert_eq!(intersection.keep_threshold, b.keep_threshold);
	assert_eq!(intersection.preserve_recent, 8);
	assert!(intersection.forbid_apply);
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
