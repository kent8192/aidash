use super::*;
#[test]
fn accepted_correction_precedes_old_prefix_without_splitting_utf8_and_default_bytes_stay_same() {
	let inputs = vec![
		AcceptedQueryInput {
			sequence: 1,
			text: "old detail".into(),
		},
		AcceptedQueryInput {
			sequence: 2,
			text: "訂正:新しい条件".into(),
		},
	];
	let old = query_material(
		QueryPolicy::TaskInputsV1,
		"Title\nDescription",
		&inputs,
		4096,
	);
	assert_eq!(old.text, "Title\nDescription\nold detail\n訂正:新しい条件");
	let recent = query_material(
		QueryPolicy::AcceptedInputsFirstV1,
		"Title\nDescription",
		&inputs,
		10,
	);
	assert_eq!(recent.text, "訂正:新");
	assert_ne!(recent.query_digest, old.query_digest);
	assert_eq!(recent.input_digest, old.input_digest);
	let mut corrected = inputs;
	corrected[1].text = "訂正:別の条件".into();
	assert_ne!(
		query_material(
			QueryPolicy::AcceptedInputsFirstV1,
			"Title\nDescription",
			&corrected,
			10
		)
		.query_digest,
		recent.query_digest
	);
}
#[test]
fn explicit_policy_rejects_duplicate_tools_and_required_home_enforcement() {
	let tool = QualifiedRef::builtin("aidash://node", "file_read");
	let mut declaration = SourceDeclaration {
		provider: "test".into(),
		operation: "read".into(),
		tools: SourceTools {
			read: vec![tool.clone()],
			detail: Some(tool.clone()),
			..Default::default()
		},
		pin: PinPolicy::Always,
		volatility: Volatility::Step,
		query: QueryPolicy::default(),
		refresh: RefreshPolicy::default(),
		disclosure: SourceDisclosure {
			previews: true,
			query: true,
		},
		required_home: true,
	};
	declaration.validate().unwrap();
	declaration.tools.read.push(tool);
	assert!(declaration.validate().is_err());
	declaration.tools.read.pop();
	declaration.pin = PinPolicy::OnDemand;
	assert!(
		declaration.validate().is_err(),
		"declared required-Home source cannot opt out of automatic contribution"
	);
	declaration.pin = PinPolicy::Scored {
		mode: Mode::Enforce,
		decider: QualifiedRef::builtin("aidash://node", "selection"),
	};
	assert!(declaration.validate().is_err());
	declaration.pin = PinPolicy::Scored {
		mode: Mode::Shadow,
		decider: QualifiedRef::builtin("aidash://node", "selection"),
	};
	declaration.validate().unwrap();
	assert!(
		SourceBatch {
			candidates: vec![],
			outcome: SourceOutcome::Delivered
		}
		.validate(
			&QualifiedRef::builtin("aidash://node", "source"),
			&declaration,
			MAX_SOURCE_BYTES
		)
		.is_err()
	);
}
#[test]
fn altered_or_partial_fitted_bytes_cannot_claim_full_delivery() {
	let source = QualifiedRef::builtin("aidash://node", "source");
	let content = json!("original full source");
	let mut contribution = SourceContribution {
		placement: None,
		mandatory: false,
		identity: CandidateIdentity {
			source,
			item: "item".into(),
			revision: "r1".into(),
			range: None,
		},
		content: content.clone(),
		provenance: vec![],
		dependencies: vec![],
		completeness: Completeness::Complete,
		detail: None,
		fetched_digest: digest(&content),
		content_digest: digest(&content),
	};
	let request = digest(&json!("accepted request"));
	assert!(accepted_delivery(&contribution, request.clone(), Uuid::nil()).is_err());
	accepted_delivery(&contribution, request.clone(), Uuid::from_u128(1)).unwrap();
	contribution.content = json!("clipped");
	contribution.content_digest = digest(&contribution.content);
	assert!(accepted_delivery(&contribution, request.clone(), Uuid::from_u128(1)).is_err());
	contribution.completeness = Completeness::Partial;
	assert!(accepted_delivery(&contribution, request, Uuid::from_u128(1)).is_err());
}

fn placed(
	item: &str,
	content: Value,
	slot: &str,
	pointer: &str,
	volatility: Option<Volatility>,
) -> SourceContribution {
	let mut contribution = SourceContribution::from(SourceCandidate {
		identity: CandidateIdentity {
			source: QualifiedRef::builtin("aidash://node", item),
			item: item.into(),
			revision: "r1".into(),
			range: None,
		},
		content,
		preview: String::new(),
		provenance: vec![crate::decision::SourcePin {
			resource: item.into(),
			revision_digest: digest(&json!(item)),
		}],
		dependencies: vec![],
		completeness: Completeness::Complete,
		detail: None,
		mandatory: false,
	});
	contribution.placement = Some(SourcePlacement {
		slot: slot.into(),
		pointer: pointer.into(),
		volatility,
	});
	contribution
}

#[test]
fn declared_mixed_sources_place_independently_and_defaults_keep_request_bytes() {
	use crate::{
		context::{Context, RequestBudget, RequestProjection},
		projection::CacheScope,
		provider::ModelContext,
	};
	let scope = CacheScope {
		tenant: "fixture".into(),
		key_version: 1,
	};
	for projection in [
		RequestProjection::Legacy,
		RequestProjection::Ordered(scope.clone()),
		RequestProjection::Native(scope),
	] {
		let pinned = json!({"reference_documents":[{"text":"step-reference"},{"text":"default-reference"}],"semantic_memory":{"matches":[{"text":"run-retrieval"}]}});
		let budget = RequestBudget {
			input_format: Default::default(),
			window: 100_000,
			instructions: "trusted instructions",
			tools: &[],
			max_output_tokens: 128,
			projection: &projection,
			estimate: crate::provider::estimate::EstimateSnapshot::conservative(0),
		};
		let default = placed(
			"default",
			pinned["reference_documents"][1].clone(),
			"references",
			"/1",
			None,
		);
		let mut context = Context {
			source_contributions: vec![default],
			..Default::default()
		};
		assert_eq!(
			serde_json::to_vec(&budget.request(&context, &pinned)).unwrap(),
			serde_json::to_vec(&budget.request(&Context::default(), &pinned)).unwrap()
		);
		let step = placed(
			"step",
			pinned["reference_documents"][0].clone(),
			"references",
			"/0",
			Some(Volatility::Step),
		);
		let run = placed(
			"run",
			pinned["semantic_memory"]["matches"][0].clone(),
			"semantic_memory",
			"/matches/0",
			Some(Volatility::Run),
		);
		context
			.source_contributions
			.extend([step.clone(), run.clone()]);
		let request = budget.request(&context, &pinned);
		assert!(
			declared_present(&request, &step),
			"{projection:?} must carry the Step declaration"
		);
		assert!(
			declared_present(&request, &run),
			"{projection:?} must carry the Run declaration"
		);
		match &request.context {
			ModelContext::Sampling(_) => panic!("expected a Run context"),
			ModelContext::Ordered(context) => {
				let stable: Value = serde_json::from_str(&context.stable).unwrap();
				assert_eq!(
					stable["reference_documents"],
					json!([null,{"text":"default-reference"}])
				);
				assert!(!context.stable.contains("step-reference"));
				assert!(context.stable.contains("run-retrieval"));
				assert!(!context.volatile.contains("run-retrieval"));
			}
			ModelContext::Native(context) => {
				assert!(
					context
						.attachments
						.iter()
						.all(|envelope| envelope.trust == crate::projection::native::Trust::Data)
				);
				assert!(context.attachments.iter().any(|envelope| envelope.source
					== crate::projection::native::Source::ReferenceDocuments
					&& envelope.content == json!([null,{"text":"default-reference"}])));
				assert!(
					!serde_json::to_string(&context.attachments)
						.unwrap()
						.contains("step-reference")
				);
				assert!(!context.volatile.contains("run-retrieval"));
			}
			ModelContext::Legacy(context) => assert_eq!(
				context["current"]["reference_documents"],
				json!([null,{"text":"default-reference"}])
			),
		}
		let mut wrong_placement = run.clone();
		wrong_placement.placement.as_mut().unwrap().volatility = Some(Volatility::Step);
		assert!(
			!declared_present(&request, &wrong_placement),
			"equal bytes at another placement cannot establish delivery"
		);
	}
}

#[test]
fn final_budget_drops_optional_envelopes_without_treating_equal_history_as_delivery() {
	use crate::{
		context::{Context, ContextEvent, RequestBudget, RequestProjection},
		projection::CacheScope,
	};
	let scope = CacheScope {
		tenant: "fixture".into(),
		key_version: 1,
	};
	for projection in [
		RequestProjection::Legacy,
		RequestProjection::Ordered(scope.clone()),
		RequestProjection::Native(scope),
	] {
		let content = "source body ".repeat(700);
		let mut item = placed(
			"private-skill",
			json!(content),
			"skills",
			"/@text/0/8400",
			Some(Volatility::Run),
		);
		let mut context = Context {
			source_contributions: vec![item.clone()],
			..Default::default()
		};
		context.push(ContextEvent::Human {
			request: "Earlier independent answer".into(),
			request_kind: "INFORMATION_REQUEST".into(),
			response: json!(content),
		});
		let budget = RequestBudget {
			input_format: Default::default(),
			window: 12_000,
			instructions: "trusted instructions",
			tools: &[],
			max_output_tokens: 128,
			projection: &projection,
			estimate: crate::provider::estimate::EstimateSnapshot::conservative(0),
		};
		let request = budget.request(&context, &json!({}));
		request.ensure_fits(12_000).unwrap();
		assert!(
			serde_json::to_string(&request.context)
				.unwrap()
				.contains(&content)
		);
		assert!(!request.instructions.contains(&content));
		assert!(
			!declared_present(&request, &item),
			"history does not establish the dropped Source's delivery"
		);
		item.mandatory = true;
		context.source_contributions = vec![item.clone()];
		let mandatory = budget.request(&context, &json!({}));
		assert!(declared_present(&mandatory, &item));
		assert!(
			mandatory.ensure_fits(12_000).is_err(),
			"mandatory receipt obligations must fail closed instead of disappearing"
		);
	}
}
