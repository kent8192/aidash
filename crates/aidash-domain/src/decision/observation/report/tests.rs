use super::*;
use chrono::Duration;
fn sample(
	at: DateTime<Utc>,
	outcome: Option<ObservationOutcome>,
	settlement: SettlementClass,
	probability: Option<f64>,
) -> ReportSample {
	ReportSample {
		approval: Uuid::new_v4(),
		created_at: at,
		group: None,
		outcome,
		settlement,
		maximum_category_probability: probability.map(|p| Probability::new(p).unwrap()),
	}
}
#[test]
fn calibration_keeps_both_false_allow_denominators_and_all_exclusions_visible() {
	let start = Utc::now();
	let range = ReportRange {
		start,
		end: start + Duration::days(1),
	};
	let allowed = Some(ObservationOutcome::Available {
		verdict: Verdict::Allowed,
	});
	let blocked = Some(ObservationOutcome::Available {
		verdict: Verdict::Blocked(BlockReason::Boundary),
	});
	let mut samples = vec![
		sample(
			start,
			allowed.clone(),
			SettlementClass::HumanAllow,
			Some(0.0),
		),
		sample(
			start,
			allowed.clone(),
			SettlementClass::HumanAllow,
			Some(0.2),
		),
		sample(
			start,
			allowed.clone(),
			SettlementClass::HumanDeny,
			Some(0.19),
		),
		sample(
			start,
			blocked.clone(),
			SettlementClass::HumanAllow,
			Some(0.6),
		),
		sample(
			start,
			blocked.clone(),
			SettlementClass::HumanDeny,
			Some(1.0),
		),
		sample(
			start,
			Some(ObservationOutcome::Unavailable {
				reason: UnavailableReason::CallsExhausted,
			}),
			SettlementClass::HumanDeny,
			None,
		),
		sample(start, None, SettlementClass::HumanAllow, None),
		sample(
			start,
			Some(ObservationOutcome::Pending),
			SettlementClass::Pending,
			None,
		),
	];
	for settlement in [
		SettlementClass::Rule,
		SettlementClass::Auto,
		SettlementClass::Bypass,
		SettlementClass::Expired,
		SettlementClass::Cancelled,
		SettlementClass::Revoked,
	] {
		samples.push(sample(start, allowed.clone(), settlement, Some(0.1)));
	}
	// Excluded by approval creation time even if its human response arrives inside range.
	samples.push(sample(
		start - Duration::milliseconds(1),
		blocked.clone(),
		SettlementClass::HumanDeny,
		Some(0.9),
	));
	samples.push(sample(
		range.end,
		blocked,
		SettlementClass::HumanDeny,
		Some(0.9),
	));
	let metrics = report(range, &samples).unwrap().metrics;
	assert_eq!(metrics.total_approvals, 14);
	assert_eq!(metrics.comparable, 5);
	assert_eq!(metrics.false_allow_among_human_denials, Rate::new(1, 2));
	assert_eq!(metrics.false_allow_among_shadow_allows, Rate::new(1, 3));
	assert_eq!(metrics.false_block_among_human_approvals, Rate::new(1, 3));
	assert_eq!(metrics.agreement, Rate::new(3, 5));
	assert_eq!(metrics.availability, Rate::new(11, 14));
	assert_eq!(metrics.label_coverage, Rate::new(7, 14));
	assert_eq!(
		metrics.exclusions,
		Exclusions {
			rule: 1,
			auto: 1,
			bypass: 1,
			expired: 1,
			cancelled: 1,
			revoked: 1,
			pending: 1,
			missing_observations: 1,
			unavailable_observations: 1,
			pending_observations: 1
		}
	);
	assert_eq!(metrics.risk_bands[0].observations, 8);
	assert_eq!(
		metrics.risk_bands[0].empirical_human_denial_rate,
		Rate::new(1, 2)
	);
	assert_eq!(metrics.risk_bands[1].observations, 1);
	assert_eq!(
		metrics.risk_bands[4].empirical_human_denial_rate,
		Rate::new(1, 1)
	);
	assert_eq!(
		metrics.risk_bands[2].empirical_human_denial_rate.value,
		None
	);
}
#[test]
fn empty_denominators_are_na_and_duplicate_approvals_cannot_inflate_report() {
	let start = Utc::now();
	let range = ReportRange {
		start,
		end: start + Duration::hours(1),
	};
	let empty = report(range.clone(), &[]).unwrap();
	assert_eq!(empty.metrics.agreement.value, None);
	assert_eq!(empty.metrics.label_coverage.value, None);
	let one = sample(start, None, SettlementClass::Auto, None);
	assert!(report(range.clone(), &[one.clone(), one]).is_err());
	assert!(
		report(
			ReportRange {
				start,
				end: start + Duration::days(32)
			},
			&[]
		)
		.is_err()
	);
	assert!(report(ReportRange { start, end: start }, &[]).is_err());
}

#[test]
fn reports_separate_effective_threshold_and_settings_revision_without_exposing_approval_ids() {
	let start = Utc::now();
	let range = ReportRange {
		start,
		end: start + Duration::hours(1),
	};
	let group = ReportGroup {
		decider: QualifiedRef {
			registry_deployment: "aidash://node".into(),
			id: "observer".into(),
			version: "1.0.0".into(),
		},
		configuration_digest: digest(&serde_json::json!("config")),
		definition_digest: digest(&serde_json::json!("definition")),
		settings_revision: 1,
		kind: ApprovalKind::WebSearch,
		operation_class: Some("read".into()),
		effect_class: Some("external_read".into()),
		route: Some(0),
		provider_contract: Some(DECISIONS.into()),
		model: Some("cloudflare/clef".into()),
		hosts: vec!["cloudflare".into()],
		served_models: vec!["cloudflare/clef".into()],
		served_hosts: vec!["cloudflare".into()],
		includes_unknown_model: false,
		includes_unknown_host: false,
		effective_threshold: Probability::new(0.3).unwrap(),
	};
	let mut rows = Vec::new();
	for (revision, threshold) in [(1, 0.3), (1, 0.3), (2, 0.3), (2, 0.2)] {
		let mut row = sample(
			start,
			Some(ObservationOutcome::Available {
				verdict: Verdict::Allowed,
			}),
			SettlementClass::HumanAllow,
			Some(0.1),
		);
		row.group = Some(ReportGroup {
			settings_revision: revision,
			effective_threshold: Probability::new(threshold).unwrap(),
			..group.clone()
		});
		rows.push(row);
	}
	let result = report(range, &rows).unwrap();
	assert_eq!(result.metrics.total_approvals, 4);
	assert_eq!(result.groups.len(), 3);
	assert_eq!(
		result
			.groups
			.iter()
			.map(|g| g.metrics.total_approvals)
			.sum::<u64>(),
		4
	);
	assert_eq!(
		result
			.groups
			.iter()
			.filter(|g| g.metrics.total_approvals == 2)
			.count(),
		1
	);
	let serialized = serde_json::to_string(&result).unwrap();
	for row in &rows {
		assert!(!serialized.contains(&row.approval.to_string()));
	}
}
