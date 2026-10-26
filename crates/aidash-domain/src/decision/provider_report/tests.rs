use super::*;
#[test]
fn reported_zero_is_distinct_from_unknown_and_invalid_costs_never_become_billable_numbers() {
	let zero = ProviderCost::new(0.0).unwrap();
	assert_eq!(
		serde_json::to_string(&zero).unwrap(),
		"\"0000000000000000\""
	);
	assert_eq!(
		serde_json::from_str::<ProviderCost>("\"3fe0000000000000\"")
			.unwrap()
			.value(),
		0.5
	);
	for invalid in [-0.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
		assert!(ProviderCost::new(invalid).is_err());
	}
	for invalid in ["\"7ff0000000000000\"", "\"bff0000000000000\"", "0"] {
		assert!(serde_json::from_str::<ProviderCost>(invalid).is_err());
	}
	let unknown = serde_json::to_value(AttemptProviderReport::Unknown).unwrap();
	let known = serde_json::to_value(AttemptProviderReport::Reported {
		served_model: None,
		served_host: None,
		input_tokens: None,
		output_tokens: None,
		cost_usd: Some(zero),
	})
	.unwrap();
	assert_eq!(unknown, serde_json::json!({"status":"unknown"}));
	assert_ne!(known, unknown);
	assert_eq!(known["cost_usd"], "0000000000000000");
}
#[test]
fn historical_attempts_default_to_unknown_and_reported_identity_must_match_the_authorized_route() {
	let historical = serde_json::json!({"id":uuid::Uuid::new_v4(),"request_digest":format!("sha256:{}","a".repeat(64)),"question_digest":format!("sha256:{}","b".repeat(64)),"questions":["q"],"status":"uncertain","owner_receipts":{"owner":uuid::Uuid::new_v4()}});
	let attempt: crate::decision::AttemptEvidence = serde_json::from_value(historical).unwrap();
	assert_eq!(attempt.provider_report, AttemptProviderReport::Unknown);
	let route = crate::decision::DeciderRoute::jev("typesafe/jev-1.14");
	let make = |model: &str, host: &str| AttemptProviderReport::Reported {
		served_model: Some(model.into()),
		served_host: Some(host.into()),
		input_tokens: Some(3),
		output_tokens: None,
		cost_usd: None,
	};
	make("typesafe/jev-1.14-20260917", "typesafe")
		.validate_route(&route)
		.unwrap();
	assert!(
		make("typesafe/jev-1.13", "typesafe")
			.validate_route(&route)
			.is_err()
	);
	assert!(
		make("typesafe/jev-1.14", "unapproved-host")
			.validate_route(&route)
			.is_err()
	);
}
