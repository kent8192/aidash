use super::*;
use rstest::rstest;

#[test]
fn a_new_tenant_starts_at_no_training_with_no_switch_on() {
	let default: DataProtection = serde_json::from_str("{}").unwrap();
	assert_eq!(default, DataProtection::default());
	assert_eq!(default.level, Level::NoTraining);
	assert!(!default.forbid_plan_connections);
	assert!(!default.forbid_keyless_endpoints);
	assert!(serde_json::from_str::<DataProtection>(r#"{"model_call_zdr":true}"#).is_err());
	assert!(serde_json::from_str::<Level>(r#""standard""#).is_err());
}

#[rstest]
#[case(Level::NoRestriction, Requirement::None, serde_json::json!({}), false)]
#[case(Level::NoTraining, Requirement::NoTraining, serde_json::json!({"data_collection":"deny"}), false)]
#[case(Level::SafetyRetention, Requirement::SafetyRetention, serde_json::json!({"zdr":true}), true)]
#[case(Level::ZeroDataRetention, Requirement::Zdr, serde_json::json!({"zdr":true}), true)]
fn each_level_asks_openrouter_for_its_own_fields(
	#[case] level: Level,
	#[case] requirement: Requirement,
	#[case] provider: serde_json::Value,
	#[case] checks_routes: bool,
) {
	let policy = DataProtection {
		level,
		..DataProtection::default()
	};
	assert_eq!(policy.requirement(), requirement);
	assert_eq!(requirement.provider(), provider);
	assert_eq!(requirement.checks_routes(), checks_routes);
	assert_eq!(
		policy.permits(Destination::OpenRouter).unwrap(),
		requirement
	);
}

#[test]
fn only_strict_zero_data_retention_excludes_safety_retention() {
	assert!(!Requirement::Zdr.admits_safety_retention());
	assert!(Requirement::SafetyRetention.admits_safety_retention());
	assert!(Requirement::default().is_default());
	assert_eq!(Requirement::default(), Requirement::Zdr);
}

#[test]
fn the_floor_and_the_chain_only_ever_raise_the_level() {
	let open = DataProtection {
		level: Level::NoRestriction,
		..DataProtection::default()
	};
	let strict = DataProtection {
		level: Level::SafetyRetention,
		forbid_plan_connections: true,
		forbid_keyless_endpoints: false,
	};
	for policies in [[open, strict], [strict, open]] {
		let combined = DataProtection::combine(policies, Level::LOWEST);
		assert_eq!(combined.level, Level::SafetyRetention);
		assert!(combined.forbid_plan_connections);
		assert!(!combined.forbid_keyless_endpoints);
	}
	assert_eq!(
		DataProtection::combine([open], Level::ZeroDataRetention).level,
		Level::ZeroDataRetention
	);
	assert_eq!(
		DataProtection::combine([strict], Level::NoTraining).level,
		Level::SafetyRetention
	);
	assert_eq!(
		DataProtection::combine([], Level::LOWEST),
		DataProtection::default()
	);
	let remote = DataProtection::combine([open, DataProtection::strictest()], Level::LOWEST);
	assert_eq!(remote, DataProtection::strictest());
}

#[rstest]
#[case(Level::NoRestriction)]
#[case(Level::NoTraining)]
#[case(Level::SafetyRetention)]
#[case(Level::ZeroDataRetention)]
fn switches_not_the_level_govern_plan_connections_and_keyless_endpoints(#[case] level: Level) {
	let allowed = DataProtection {
		level,
		..DataProtection::default()
	};
	assert!(allowed.permits(Destination::PlanConnection).is_ok());
	assert!(allowed.permits(Destination::TenantKeyless).is_ok());
	assert!(allowed.permits(Destination::DeploymentHosted).is_ok());
	assert!(allowed.permits(Destination::TenantTool).is_ok());
	let forbidden = DataProtection {
		level,
		forbid_plan_connections: true,
		forbid_keyless_endpoints: true,
	};
	assert_eq!(
		forbidden.refusal(Destination::PlanConnection),
		Some(Refusal::PlanConnectionsForbidden)
	);
	assert_eq!(
		forbidden.refusal(Destination::TenantKeyless),
		Some(Refusal::KeylessEndpointsForbidden)
	);
	assert!(forbidden.permits(Destination::PlanConnection).is_err());
	assert!(forbidden.permits(Destination::DeploymentHosted).is_ok());
	assert_eq!(
		allowed.permits(Destination::ProviderTool).is_ok(),
		level == Level::NoRestriction
	);
}

fn catalog() -> Vec<serde_json::Value> {
	use serde_json::json;
	vec![
		json!({"model_id":"vendor/model","tag":"strict","safety_retention":false}),
		json!({"model_id":"vendor/model","tag":"kept","safety_retention":true}),
		// A missing or non-boolean field counts as Safety Retention.
		json!({"model_id":"vendor/model","tag":"unmarked"}),
		json!({"model_id":"vendor/model","tag":"odd","safety_retention":"no"}),
		// `host` is strict but sits above a Safety Retention tag.
		json!({"model_id":"vendor/model","tag":"host","safety_retention":false}),
		json!({"model_id":"vendor/model","tag":"host/eu","safety_retention":true}),
		// The same tag is Safety Retention only for another model.
		json!({"model_id":"vendor/other","tag":"strict","safety_retention":true}),
		json!({"model_id":"google/kept-only","tag":"google-vertex","safety_retention":true}),
	]
}

#[test]
fn strict_zero_data_retention_drops_safety_retention_per_model_and_parent_tags() {
	let catalog = catalog();
	assert_eq!(
		admitted_routes(&catalog, "vendor/model", Requirement::Zdr).unwrap(),
		["strict"]
	);
	assert_eq!(
		admitted_routes(&catalog, "vendor/model", Requirement::SafetyRetention).unwrap(),
		["host", "host/eu", "kept", "odd", "strict", "unmarked"]
	);
	assert_eq!(
		admitted_routes(&catalog, "vendor/other", Requirement::Zdr),
		Err(CatalogRefusal::SafetyRetentionOnly)
	);
	assert_eq!(
		admitted_routes(&catalog, "google/kept-only", Requirement::Zdr),
		Err(CatalogRefusal::SafetyRetentionOnly)
	);
	assert_eq!(
		admitted_routes(&catalog, "google/kept-only", Requirement::SafetyRetention).unwrap(),
		["google-vertex"]
	);
	assert_eq!(
		admitted_routes(&catalog, "vendor/absent", Requirement::Zdr),
		Err(CatalogRefusal::NoRoute)
	);
}

#[test]
fn a_pinned_safety_retention_route_has_its_own_reason() {
	let catalog = catalog();
	assert_eq!(
		pinned_route_refusal(&catalog, "vendor/model", "kept", Requirement::Zdr),
		Some(PinnedRefusal::SafetyRetentionOnly)
	);
	assert_eq!(
		pinned_route_refusal(
			&catalog,
			"vendor/model",
			"kept",
			Requirement::SafetyRetention
		),
		None
	);
	assert_eq!(
		pinned_route_refusal(&catalog, "vendor/model", "strict", Requirement::Zdr),
		None
	);
	assert_eq!(
		pinned_route_refusal(&catalog, "vendor/model", "missing", Requirement::Zdr),
		Some(PinnedRefusal::NotListed)
	);
}

#[test]
fn only_meets_requires_a_nonempty_list_of_admitted_routes() {
	use serde_json::json;
	let catalog = catalog();
	let meets = |only: Option<serde_json::Value>| {
		only_meets(only.as_ref(), &catalog, "vendor/model", Requirement::Zdr)
	};
	assert!(meets(Some(json!(["strict"]))));
	assert!(!meets(None));
	assert!(!meets(Some(json!([]))));
	assert!(!meets(Some(json!(["strict", "kept"]))));
	assert!(!meets(Some(json!(["host"]))));
	assert!(!meets(Some(json!("strict"))));
}
