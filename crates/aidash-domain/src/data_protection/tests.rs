use super::*;

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
		assert!(combined.permits(Destination::DeploymentHosted).is_ok());
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
