use super::*;

#[test]
fn residency_is_bound_by_the_deployment_and_intersects_across_tenants() {
	let restricted = DataProtection {
		data_region_restricted: true,
		..Default::default()
	};
	assert!(
		restricted
			.in_deployment(None)
			.permits(Destination::OpenRouter)
			.is_err()
	);
	let eu = restricted.in_deployment(Some(Region::Eu));
	let us = restricted.in_deployment(Some(Region::Us));
	for policies in [[eu, us], [us, eu]] {
		assert!(
			DataProtection::combine(policies, Level::Standard)
				.permits(Destination::OpenRouter)
				.unwrap_err()
				.to_string()
				.contains("do not overlap")
		);
	}
	for policies in [
		[eu, DataProtection::default()],
		[DataProtection::default(), eu],
		[eu, eu],
	] {
		let combined = DataProtection::combine(policies, Level::Standard);
		assert_eq!(combined.region().unwrap(), Some(Region::Eu));
		assert!(
			combined
				.permits(Destination::DeploymentHosted(Some(Region::Eu)))
				.is_ok()
		);
		assert!(
			combined
				.permits(Destination::DeploymentHosted(Some(Region::Us)))
				.is_err()
		);
		assert!(combined.permits(Destination::TenantKeyless).is_err());
		assert!(
			combined
				.permits(Destination::PlanConnection)
				.unwrap()
				.plan_connection_exemption
		);
		assert!(
			DataProtection {
				plan_connection_exempt: false,
				..combined
			}
			.permits(Destination::PlanConnection)
			.is_err()
		);
	}
	assert!(
		restricted
			.in_deployment(Some(Region::Jp))
			.permits(Destination::OpenRouter)
			.unwrap_err()
			.to_string()
			.contains("no JP endpoint")
	);
}

#[test]
fn residency_requires_matching_research_terms_and_does_not_change_legacy_bytes() {
	let mut terms: crate::research::account::AccountTerms =
		serde_json::from_value(serde_json::json!({
			"account_id":"research","agreement_reference":"agreement","verified_by":"admin",
			"verified_at":"2026-10-01T00:00:00Z","review_after":"2026-12-01T00:00:00Z",
			"storage_permitted":true,"query_training_prohibited":true,"evaluation_permitted":true,
			"retention_days":7,"deletion_after_termination_days":7,
			"price_effective_at":"2026-10-01T00:00:00Z","price_review_after":"2026-12-01T00:00:00Z",
			"monthly_base_fee_micro_usd":0,"monthly_limit_micro_usd":100
		}))
		.unwrap();
	let policy = DataProtection {
		data_region_restricted: true,
		..Default::default()
	}
	.in_deployment(Some(Region::Us));
	for region in [None, Some(Region::Eu), Some(Region::Us)] {
		terms.data_region = region;
		assert_eq!(
			policy.permits(Destination::Research(&terms)).is_ok(),
			region == Some(Region::Us)
		);
	}
	assert_eq!(
		serde_json::to_value(DataProtection::default()).unwrap(),
		serde_json::json!({"level":"standard","model_call_zdr":true,"plan_connection_exempt":true})
	);
	assert!(serde_json::from_str::<Region>("\"arbitrary-host\"").is_err());
}

#[test]
fn legacy_defaults_and_explicit_opt_out_have_distinct_wire_requirements() {
	let default: DataProtection = serde_json::from_str("{}").unwrap();
	assert_eq!(default, DataProtection::default());
	assert_eq!(
		default.requirement().provider(),
		serde_json::json!({"zdr":true})
	);
	let mut policy = DataProtection {
		model_call_zdr: false,
		..default
	};
	assert_eq!(policy.requirement().provider(), serde_json::json!({}));
	policy.level = Level::NoTraining;
	assert_eq!(
		policy.requirement().provider(),
		serde_json::json!({"data_collection":"deny"})
	);
	policy.level = Level::ZeroDataRetention;
	assert!(policy.validate().is_err());
	assert_eq!(
		policy.with_floor(Level::ZeroDataRetention).requirement(),
		Requirement::Zdr
	);
}

#[test]
fn delegation_never_weakens_any_tenants_requirements() {
	let opted_out = DataProtection {
		level: Level::NoTraining,
		model_call_zdr: false,
		plan_connection_exempt: false,
		..Default::default()
	};
	let default = DataProtection::default();
	for policies in [[opted_out, default], [default, opted_out]] {
		let combined = DataProtection::combine(policies, Level::Standard);
		assert_eq!(combined.level, Level::NoTraining);
		assert_eq!(combined.requirement(), Requirement::Zdr);
		assert!(!combined.plan_connection_exempt);
		assert!(combined.permits(Destination::PlanConnection).is_err());
		assert!(combined.permits(Destination::TenantKeyless).is_err());
		assert!(combined.permits(Destination::ProviderTool).is_err());
		assert!(
			combined
				.permits(Destination::DeploymentHosted(None))
				.is_ok()
		);
		assert!(combined.permits(Destination::TenantTool).is_ok());
	}
	let floored = DataProtection::combine([opted_out], Level::ZeroDataRetention);
	assert_eq!(floored.requirement(), Requirement::Zdr);
	assert_eq!(floored.level, Level::ZeroDataRetention);
	assert!(
		DataProtection {
			plan_connection_exempt: true,
			..floored
		}
		.permits(Destination::PlanConnection)
		.unwrap()
		.plan_connection_exemption
	);
}

#[test]
fn non_exempt_plan_connections_are_allowed_only_at_standard_even_with_model_zdr() {
	for level in [Level::Standard, Level::NoTraining, Level::ZeroDataRetention] {
		for model_call_zdr in [false, true] {
			let policy = DataProtection {
				level,
				model_call_zdr,
				plan_connection_exempt: false,
				..Default::default()
			}
			.with_floor(Level::Standard);
			let permission = policy.permits(Destination::PlanConnection);
			if level == Level::Standard {
				assert!(!permission.unwrap().plan_connection_exemption);
			} else {
				assert!(permission.is_err());
			}
		}
	}
}
