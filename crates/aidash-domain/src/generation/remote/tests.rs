use super::*;
use crate::registry::EntityRef;
use rstest::{fixture, rstest};

#[fixture]
fn provider() -> Provider {
	Provider {
		node_id: "aidash://edge".into(),
		entry: EntityRef {
			id: "emb".into(),
			version: "1.2.3".into(),
		},
		digest: format!("sha256:{}", "a".repeat(64)),
		configuration_digest: format!("sha256:{}", "b".repeat(64)),
	}
}

#[fixture]
fn usage(provider: Provider) -> Usage {
	Usage {
		operation_id: Uuid::from_u128(1),
		attempt_id: Uuid::from_u128(2),
		dispatcher_node: "aidash://edge".into(),
		grant_id: Uuid::from_u128(3),
		admission_id: Uuid::from_u128(4),
		purpose: Purpose::Embedding,
		provider,
		input_digest: format!("sha256:{}", "c".repeat(64)),
		reserved_tokens: 1024,
	}
}

#[rstest]
fn usage_has_the_existing_canonical_wire_digest(usage: Usage) {
	usage.validate().unwrap();
	assert_eq!(
		usage.digest().unwrap(),
		"sha256:9904b0fe09fce025002c4ee312a584a60c589039cd31cfa6c2d424c12e413fe4"
	);
	let wire = serde_json::to_value(&usage).unwrap();
	assert_eq!(wire["purpose"], "embedding");
	assert_eq!(
		serde_json::to_value(serde_json::from_value::<Usage>(wire.clone()).unwrap()).unwrap(),
		wire
	);
}

#[rstest]
#[case("operation_id", serde_json::json!(Uuid::nil()))]
#[case("attempt_id", serde_json::json!(Uuid::nil()))]
#[case("grant_id", serde_json::json!(Uuid::nil()))]
#[case("admission_id", serde_json::json!(Uuid::nil()))]
#[case("reserved_tokens", serde_json::json!(0))]
#[case("reserved_tokens", serde_json::json!(1_000_000_000_001_i64))]
#[case("input_digest", serde_json::json!("sha256:short"))]
#[case("input_digest", serde_json::json!(format!("sha256:{}", "g".repeat(64))))]
#[case("dispatcher_node", serde_json::json!("aidash://other"))]
fn invalid_usage_retains_the_provider_contract_recovery_reason(
	usage: Usage,
	#[case] field: &str,
	#[case] value: serde_json::Value,
) {
	let mut wire = serde_json::to_value(usage).unwrap();
	wire[field] = value;
	let changed: Usage = serde_json::from_value(wire).unwrap();
	assert!(matches!(
		changed.validate(),
		Err(ContractError::Semantic(Failure::ProviderContract))
	));
}

#[rstest]
fn a_malformed_matching_dispatcher_is_a_domain_validation_error(mut usage: Usage) {
	usage.dispatcher_node = "bad/node".into();
	usage.provider.node_id.clone_from(&usage.dispatcher_node);
	assert!(matches!(
		usage.validate(),
		Err(ContractError::Domain(crate::Error::Invalid(_)))
	));
}

#[rstest]
#[case(1)]
#[case(1_000_000_000_000)]
fn usage_accepts_the_existing_token_budget_bounds(mut usage: Usage, #[case] tokens: i64) {
	usage.reserved_tokens = tokens;
	usage.validate().unwrap();
}

#[rstest]
#[case("calls_per_agent", serde_json::json!(0))]
#[case("calls_per_agent", serde_json::json!(4))]
#[case("call_budget", serde_json::json!(0))]
#[case("call_budget", serde_json::json!(1_000_001))]
fn approvals_reject_overallocated_provider_calls(
	provider: Provider,
	#[case] field: &str,
	#[case] value: serde_json::Value,
) {
	let mut wire = serde_json::json!({"embedding":{"provider": provider, "calls_per_agent":2, "call_budget":3}});
	wire["embedding"][field] = value;
	let approvals: Approvals = serde_json::from_value(wire).unwrap();
	assert!(matches!(
		approvals.validate(),
		Err(crate::Error::Invalid(_))
	));
}

#[rstest]
fn approvals_retain_default_omission_and_the_inference_frontier(provider: Provider) {
	let empty = Approvals::default();
	assert_eq!(serde_json::to_value(&empty).unwrap(), serde_json::json!({}));
	empty.validate().unwrap();
	let mut approvals = Approvals {
		inference: vec![provider; 32],
		..Default::default()
	};
	approvals.validate().unwrap();
	approvals.inference.push(approvals.inference[0].clone());
	assert!(matches!(
		approvals.validate(),
		Err(crate::Error::Invalid(_))
	));
}

#[rstest]
#[case("node_id", serde_json::json!("edge"))]
#[case("digest", serde_json::json!("sha256:short"))]
#[case("configuration_digest", serde_json::json!("sha256:short"))]
fn approvals_require_exact_provider_identity_and_digests(
	provider: Provider,
	#[case] field: &str,
	#[case] value: serde_json::Value,
) {
	let mut wire = serde_json::to_value(provider).unwrap();
	wire[field] = value;
	let approvals = Approvals {
		inference: vec![serde_json::from_value(wire).unwrap()],
		..Default::default()
	};
	assert!(matches!(
		approvals.validate(),
		Err(crate::Error::Invalid(_))
	));
}

fn owners() -> Vec<Ancestor> {
	[10, 11]
		.into_iter()
		.map(|id| Ancestor {
			node_id: "aidash://origin".into(),
			tenant: "acme".into(),
			request_id: Uuid::from_u128(id),
			policy_id: "policy".into(),
			policy_revision: 2,
			depth: 1,
			expires_at: chrono::DateTime::parse_from_rfc3339("2026-10-03T00:00:00Z")
				.unwrap()
				.with_timezone(&Utc),
		})
		.collect()
}

#[rstest]
fn receipts_match_the_exact_ordered_lineage_and_attempt(usage: Usage) {
	let expected = owners();
	let receipts = expected
		.iter()
		.cloned()
		.map(|owner| Reserved {
			owner,
			attempt_id: usage.attempt_id,
			digest: usage.digest().unwrap(),
		})
		.collect::<Vec<_>>();
	verify_receipts(&expected, &receipts, &usage).unwrap();
	verify_receipts(&[], &[], &usage).unwrap();
}

#[rstest]
#[case("missing")]
#[case("extra")]
#[case("reordered")]
#[case("attempt")]
#[case("digest")]
#[case("owner_revision")]
#[case("owner_expiration")]
fn receipts_reject_incomplete_or_rebound_reservations(usage: Usage, #[case] mutation: &str) {
	let expected = owners();
	let mut receipts = expected
		.iter()
		.cloned()
		.map(|owner| Reserved {
			owner,
			attempt_id: usage.attempt_id,
			digest: usage.digest().unwrap(),
		})
		.collect::<Vec<_>>();
	match mutation {
		"missing" => {
			receipts.pop();
		}
		"extra" => receipts.push(receipts[0].clone()),
		"reordered" => receipts.swap(0, 1),
		"attempt" => receipts[0].attempt_id = Uuid::from_u128(99),
		"digest" => receipts[0].digest = format!("sha256:{}", "e".repeat(64)),
		"owner_revision" => receipts[0].owner.policy_revision += 1,
		"owner_expiration" => receipts[0].owner.expires_at += chrono::Duration::seconds(1),
		_ => panic!("unknown fixture mutation"),
	}
	assert!(matches!(
		verify_receipts(&expected, &receipts, &usage),
		Err(ContractError::Semantic(Failure::ProviderContract))
	));
}

#[rstest]
#[case(Purpose::Embedding, "embedding", "embedding.invoke")]
#[case(Purpose::Compaction, "compaction", "compaction.invoke")]
#[case(Purpose::Inference, "inference", "model.infer")]
fn purpose_preserves_wire_names_and_authorization_actions(
	#[case] purpose: Purpose,
	#[case] name: &str,
	#[case] action: &str,
) {
	assert_eq!(serde_json::to_value(purpose).unwrap(), name);
	assert_eq!(purpose.name(), name);
	assert_eq!(purpose.action(), action);
}

#[rstest]
#[case(serde_json::json!({"state":"settled","reported":null}))]
#[case(serde_json::json!({"state":"settled","reported":17}))]
#[case(serde_json::json!({"state":"aborted"}))]
fn finalization_retains_closed_tagged_wire_states(#[case] wire: serde_json::Value) {
	let decoded: Finalization = serde_json::from_value(wire.clone()).unwrap();
	assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
}

#[rstest]
#[case(serde_json::json!({"state":"unknown"}))]
#[case(serde_json::json!({"state":"aborted","reported":1}))]
fn finalization_rejects_unknown_or_mixed_states(#[case] wire: serde_json::Value) {
	assert!(serde_json::from_value::<Finalization>(wire).is_err());
}
