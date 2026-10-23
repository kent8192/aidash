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
