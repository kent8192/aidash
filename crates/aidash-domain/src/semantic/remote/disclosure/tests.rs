use super::*;
use crate::semantic::{
	indexing::content_digest,
	remote::Binding,
	results::{Match, SearchResult},
};
use chrono::{TimeZone, Utc};
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;
fn receipt(matches: usize) -> Receipt {
	Receipt {
		memory: None,
		operation_id: Uuid::from_u128(1),
		operation_digest: "digest".into(),
		home_node: "aidash://home".into(),
		tenant: "tenant".into(),
		workspace_id: Uuid::from_u128(2),
		grant_id: Uuid::from_u128(3),
		admission_id: Uuid::from_u128(4),
		executor: "aidash://receiver/agent@1".into(),
		binding: Binding::Disabled {},
		retrieved_at: Utc.timestamp_opt(0, 0).unwrap(),
		query_truncated: false,
		candidate_digest: "candidates".into(),
		sources: vec![],
		result: SearchResult {
			workspace_id: Uuid::from_u128(2),
			index_revision: 7,
			model: "embedding".into(),
			model_version: "1".into(),
			matches: (0..matches)
				.map(|i| Match {
					entry_id: Uuid::from_u128(10 + i as u128),
					revision: i as i64 + 1,
					source: json!({"kind":"memory"}),
					agent: None,
					metadata: json!({}),
					text: format!("source {i} 日本語"),
					score: 0.75,
				})
				.collect(),
			estimated_tokens: 0,
			truncated: false,
		},
		estimated_tokens: 0,
	}
}
#[rstest]
fn full_receipt_budgets_serialized_provenance_and_exact_utf8_source_digests() {
	let mut value = receipt(2);
	let result_estimate = crate::semantic::retrieval::result_tokens(&value.result).unwrap();
	value.fit_budget(32768).unwrap();
	assert_eq!(value.result.estimated_tokens, result_estimate);
	assert_eq!(value.sources.len(), 2);
	for (source, matched) in value.sources.iter().zip(&value.result.matches) {
		assert_eq!(source.entry_id, matched.entry_id);
		assert_eq!(source.revision, matched.revision);
		assert_eq!(source.content_digest, content_digest(&matched.text));
	}
	// The existing estimate includes sixteen tokens of room for its own counter to grow.
	let mut before_counter = value.clone();
	before_counter.estimated_tokens = 0;
	assert_eq!(
		value.estimated_tokens,
		crate::context::estimated_tokens(&serde_json::to_string(&before_counter).unwrap()) + 16
	);
	assert!(!value.result.truncated);
}
#[rstest]
fn truncation_preserves_the_first_match_and_removes_trailing_source_provenance() {
	let mut minimum = receipt(1);
	minimum.result.truncated = true;
	minimum.fit_budget(32768).unwrap();
	let budget = minimum.estimated_tokens + 16;
	let mut value = receipt(3);
	value.fit_budget(budget).unwrap();
	assert_eq!(value.result.matches.len(), 1);
	assert_eq!(value.sources.len(), 1);
	assert_eq!(value.result.matches[0].entry_id, Uuid::from_u128(10));
	assert_eq!(value.sources[0].entry_id, Uuid::from_u128(10));
	assert!(value.result.truncated);
	assert!(value.estimated_tokens <= budget);
}
#[rstest]
#[case::original_match(1, false)]
#[case::previously_truncated(0, true)]
fn a_retrieval_with_candidates_cannot_become_an_empty_success(
	#[case] matches: usize,
	#[case] truncated: bool,
) {
	let mut value = receipt(matches);
	value.result.truncated = truncated;
	let mut empty = receipt(0);
	empty.fit_budget(32768).unwrap();
	// A genuine empty receipt establishes the wire budget without discarding candidates.
	let empty_budget = empty.estimated_tokens + 16;
	assert!(matches!(
		value.fit_budget(empty_budget),
		Err(ContractError::Semantic(Failure::ContextBudget))
	));
}
#[rstest]
fn genuine_empty_candidate_set_can_return_provenance_without_inventing_sources() {
	let mut value = receipt(0);
	value.fit_budget(32768).unwrap();
	assert!(value.sources.is_empty());
	assert!(value.result.matches.is_empty());
	assert!(!value.result.truncated);
	assert!(value.estimated_tokens > value.result.estimated_tokens);
}
#[rstest]
fn budget_too_small_for_receipt_provenance_is_rejected() {
	let mut value = receipt(0);
	assert!(matches!(
		value.fit_budget(0),
		Err(ContractError::Semantic(Failure::ContextBudget))
	));
}

#[rstest]
#[case(0)]
#[case(1)]
fn mixed_receipt_preserves_ordinary_provenance_before_trimming_native_units(
	#[case] retained: usize,
) {
	use crate::registry::EntityRef;
	use crate::semantic::remote::{
		NativeBank, NativeBinding, NativeContext, NativeRecall, NativeRequest, Provider,
	};
	let mut value = receipt(1);
	let reference = |id: &str| EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	};
	let bank = crate::memory::Bank {
		home: value.home_node.clone(),
		tenant: value.tenant.clone(),
		workspace: value.workspace_id,
		participant: Some(Uuid::from_u128(100)),
	};
	let provider = Provider {
		node_id: value.home_node.clone(),
		entry: reference("memory"),
		digest: "provider".into(),
		configuration_digest: "configuration".into(),
	};
	value.binding = Binding::RequiredHome {
		home_lineage: vec![],
		execution_lineage: vec![],
		version: 1,
		index_revision: 0,
		index_digest: "index".into(),
		embedding: Box::new(provider.clone()),
		compactor: None,
		summarizer: None,
		native: Some(Box::new(NativeBinding {
			selection: NativeRequest {
				participant: bank.participant.unwrap(),
				expected_revision: 1,
				provider: provider.entry.clone(),
			},
			generation: None,
			participant: crate::memory::Binding {
				bank: bank.clone(),
				participant_revision: 1,
				agent: reference("agent"),
				provider: provider.entry.clone(),
			},
			agent: provider.clone(),
			banks: vec![NativeBank {
				bank: bank.clone(),
				provider: provider.clone(),
				roles: vec![],
				max_model_tokens: 8192,
				max_context_tokens: 8192,
				cache_max_age_seconds: 60,
				cache_max_attempts: 2,
			}],
		})),
	};
	let units: Vec<crate::memory::Unit> = (0..3).map(|index| serde_json::from_value(json!({"id":Uuid::from_u128(110+index),"bank":bank,"revision":1,
        "content":{"text":"native quotation / 記憶 ".repeat(100),"kind":"world","learning":"fact","verification":"unverified","mental_model":null,"occurred":null,"entities":[],"evidence":[],"links":[]},
        "learned_at":value.retrieved_at,"updated_at":value.retrieved_at,"deleted":false,"stale":false})).unwrap()).collect();
	value.memory = Some(NativeContext {
		banks: vec![NativeRecall {
			bank,
			provider: provider.entry,
			recall: crate::memory::Recall::Ready {
				units: units.clone(),
			},
		}],
	});
	value.validate_native().unwrap();
	let ordinary = value.result.matches[0].clone();
	let mut minimum = value.clone();
	minimum.memory.as_mut().unwrap().banks[0].recall = if retained == 0 {
		crate::memory::Recall::NoSpace
	} else {
		crate::memory::Recall::Ready {
			units: units[..retained].to_vec(),
		}
	};
	minimum.fit_budget(100_000).unwrap();
	let budget = minimum.estimated_tokens + 16;
	value.fit_budget(budget).unwrap();
	value.validate_native().unwrap();
	assert_eq!(
		serde_json::to_value(&value.result.matches).unwrap(),
		json!([ordinary])
	);
	assert_eq!(value.sources.len(), 1);
	assert_eq!(
		value.sources[0].content_digest,
		content_digest(&ordinary.text)
	);
	assert!(value.estimated_tokens <= budget);
	assert_eq!(
		value.memory.as_ref().unwrap().banks[0].recall,
		minimum.memory.as_ref().unwrap().banks[0].recall
	);
}
