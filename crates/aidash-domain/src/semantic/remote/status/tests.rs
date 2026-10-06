use super::*;
use crate::registry::EntityRef;
use crate::semantic::{
	remote::Provider,
	results::{Match, SearchResult},
};
use rstest::{fixture, rstest};
use serde_json::{Value, json};

#[fixture]
fn binding() -> Binding {
	Binding::RequiredHome {
		native: None,
		home_lineage: vec![],
		execution_lineage: vec![],
		version: 1,
		index_revision: 7,
		index_digest: "index-digest".into(),
		embedding: Box::new(Provider {
			node_id: "home".into(),
			entry: EntityRef {
				id: "embedding".into(),
				version: "1".into(),
			},
			digest: "provider-digest".into(),
			configuration_digest: "configuration-digest".into(),
		}),
		compactor: None,
	}
}
#[fixture]
fn receipt(binding: Binding) -> Receipt {
	let entry = Uuid::from_u128(1);
	Receipt {
		memory: None,
		operation_id: Uuid::from_u128(2),
		operation_digest: "operation-digest".into(),
		home_node: "home".into(),
		tenant: "tenant".into(),
		workspace_id: Uuid::from_u128(3),
		grant_id: Uuid::from_u128(4),
		admission_id: Uuid::from_u128(5),
		executor: "receiver/agent@1".into(),
		binding,
		retrieved_at: DateTime::from_timestamp(1000, 0).unwrap(),
		query_truncated: false,
		candidate_digest: "candidate-digest".into(),
		sources: vec![super::super::SourceRead {
			entry_id: entry,
			revision: 8,
			content_digest: "source-digest".into(),
		}],
		result: SearchResult {
			workspace_id: Uuid::from_u128(3),
			index_revision: 7,
			model: "embedding".into(),
			model_version: "1".into(),
			matches: vec![Match {
				entry_id: entry,
				revision: 8,
				source: json!({"text":"private source"}),
				agent: Some("agent".into()),
				metadata: Value::Null,
				text: "private result text".into(),
				score: 0.9,
			}],
			estimated_tokens: 10,
			truncated: false,
		},
		estimated_tokens: 10,
	}
}
#[fixture]
fn record(receipt: Receipt) -> Record {
	Record {
		id: receipt.operation_id,
		home_node: receipt.home_node.clone(),
		grant_id: receipt.grant_id,
		admission_id: receipt.admission_id,
		digest: receipt.operation_digest.clone(),
		binding: Value::Null,
		state: "READY".into(),
		cycle: 2,
		failures: 9,
		attempt_id: None,
		fence: 7,
		lease_until: None,
		next_attempt: None,
		error: None,
		receipt: Some(json!(receipt)),
	}
}
#[rstest]
#[case(0, false, false, "empty")]
#[case(1, false, false, "ready")]
#[case(1, true, false, "truncated")]
#[case(1, false, true, "truncated")]
fn receipt_summary_keeps_counts_and_both_truncation_flags(
	binding: Binding,
	mut receipt: Receipt,
	mut record: Record,
	#[case] count: usize,
	#[case] query: bool,
	#[case] result: bool,
	#[case] state: &str,
) {
	receipt.result.matches.truncate(count);
	receipt.query_truncated = query;
	receipt.result.truncated = result;
	record.receipt = Some(json!(receipt));
	let view = project(&binding, None, Some(record)).unwrap();
	assert_eq!(view.state, state);
	assert_eq!(view.result_count, Some(count));
	assert_eq!(view.retry_count, 5);
	assert_eq!(view.truncated, query || result);
	assert_eq!(view.retrieved_at, Some(receipt.retrieved_at));
}
#[rstest]
#[case(Failure::Invalidated, false, "invalidated")]
#[case(Failure::Authority, false, "paused")]
#[case(Failure::Unavailable, true, "waiting")]
fn current_failure_prevents_receipt_decoding_or_disclosure(
	binding: Binding,
	mut record: Record,
	#[case] reason: Failure,
	#[case] retry: bool,
	#[case] state: &str,
) {
	record.receipt = Some(json!({"private":"malformed receipt"}));
	record.next_attempt = retry.then(|| DateTime::from_timestamp(2000, 0).unwrap());
	let view = project(&binding, Some(reason), Some(record)).unwrap();
	assert_eq!(view.state, state);
	assert_eq!(view.reason, Some(reason));
	assert_eq!(view.result_count, None);
	assert_eq!(view.retrieved_at, None);
	assert!(!view.truncated);
}
#[rstest]
fn disabled_and_unstarted_bindings_keep_their_original_initial_state(
	binding: Binding,
	mut record: Record,
) {
	record.receipt = Some(Value::Null);
	let disabled = project(
		&Binding::Disabled {},
		Some(Failure::Authority),
		Some(record),
	)
	.unwrap();
	assert_eq!(disabled.state, "disabled");
	assert_eq!(disabled.operation_id, None);
	assert_eq!(disabled.reason, Some(Failure::Authority));
	let pending = project(&binding, None, None).unwrap();
	assert_eq!(pending.state, "pending");
	assert_eq!(pending.retry_count, 0);
}
#[rstest]
fn provenance_keeps_identity_and_digests_without_source_or_result_text(receipt: Receipt) {
	let view = Provenance::from(receipt.clone());
	assert_eq!(view.home_node, receipt.home_node);
	assert_eq!(view.sources.len(), 1);
	assert_eq!(view.sources[0].entry_id, receipt.sources[0].entry_id);
	assert_eq!(view.sources[0].revision, 8);
	assert_eq!(view.sources[0].content_digest, "source-digest");
	assert_eq!(view.sources[0].agent.as_deref(), Some("agent"));
	assert!(view.allowances.is_empty());
	assert_eq!(view.allowance_node, "");
	let value = json!(view);
	assert!(value.get("query").is_none());
	assert!(value["sources"][0].get("source").is_none());
	assert!(value["sources"][0].get("text").is_none());
	assert!(!value.to_string().contains("private"));
}
#[rstest]
fn malformed_ready_receipt_remains_a_contract_error(binding: Binding, mut record: Record) {
	record.receipt = Some(json!({"invalid":"receipt"}));
	assert!(project(&binding, None, Some(record)).is_err());
}
#[rstest]
#[case(false, "empty")]
#[case(true, "no_space")]
fn native_summary_distinguishes_empty_and_no_space(
	binding: Binding,
	mut receipt: Receipt,
	mut record: Record,
	#[case] no_space: bool,
	#[case] expected: &str,
) {
	receipt.result.matches.clear();
	receipt.sources.clear();
	receipt.memory = Some(super::super::NativeContext {
		banks: vec![super::super::NativeRecall {
			bank: crate::memory::Bank {
				home: receipt.home_node.clone(),
				tenant: receipt.tenant.clone(),
				workspace: receipt.workspace_id,
				participant: None,
			},
			provider: EntityRef {
				id: "native".into(),
				version: "1.0.0".into(),
			},
			recall: if no_space {
				crate::memory::Recall::NoSpace
			} else {
				crate::memory::Recall::Empty
			},
		}],
	});
	record.receipt = Some(json!(receipt));
	let status = project(&binding, None, Some(record)).unwrap();
	assert_eq!(status.state, expected);
	assert_eq!(status.result_count, Some(0));
}
#[rstest]
fn native_provenance_retains_exact_support_without_claim_bodies(mut receipt: Receipt) {
	let bank = crate::memory::Bank {
		home: receipt.home_node.clone(),
		tenant: receipt.tenant.clone(),
		workspace: receipt.workspace_id,
		participant: None,
	};
	let unit: crate::memory::Unit = serde_json::from_value(json!({"id":Uuid::from_u128(100),"bank":bank,"revision":7,
        "content":{"text":"private native claim","kind":"world","learning":"fact","verification":"unverified","mental_model":null,"occurred":null,"entities":[],"evidence":[{"kind":"artifact","id":Uuid::from_u128(101),"revision":2,"digest":"source-digest"}],"links":[]},
        "learned_at":receipt.retrieved_at,"updated_at":receipt.retrieved_at,"deleted":false,"stale":false})).unwrap();
	let digest = crate::registry::rules::digest(&json!(unit.content));
	receipt.memory = Some(super::super::NativeContext {
		banks: vec![super::super::NativeRecall {
			bank,
			provider: EntityRef {
				id: "native".into(),
				version: "1.0.0".into(),
			},
			recall: crate::memory::Recall::Ready { units: vec![unit] },
		}],
	});
	let provenance = Provenance::from(receipt);
	assert_eq!(provenance.memory[0].units[0].revision, 7);
	assert_eq!(
		provenance.memory[0].units[0].verification,
		crate::memory::Verification::Unverified
	);
	assert_eq!(provenance.memory[0].units[0].evidence.len(), 1);
	assert_eq!(provenance.memory[0].units[0].content_digest, digest);
	assert!(
		!json!(provenance)
			.to_string()
			.contains("private native claim")
	);
}
