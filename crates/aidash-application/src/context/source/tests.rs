use super::*;
use aidash_domain::{decision::SourcePin, registry::bindings::QualifiedRef};
fn candidate(source: &str, revision: &str, dependency: &str, mandatory: bool) -> SourceCandidate {
	let pin = SourcePin {
		resource: dependency.into(),
		revision_digest: digest(&json!(dependency)),
	};
	SourceCandidate {
		identity: CandidateIdentity {
			source: QualifiedRef::builtin("aidash://node", source),
			item: "item".into(),
			revision: revision.into(),
			range: None,
		},
		content: json!("equal text"),
		preview: "preview".into(),
		provenance: vec![pin.clone()],
		dependencies: vec![pin],
		completeness: Completeness::Complete,
		detail: None,
		mandatory,
	}
}
#[test]
fn dedup_preserves_authority_and_equal_text_from_distinct_sources_or_revisions() {
	let candidates = vec![
		candidate("a", "1", "first", false),
		candidate("a", "1", "second", true),
		candidate("b", "1", "other-source", false),
		candidate("a", "2", "other-revision", false),
	];
	let merged = deduplicate(candidates).unwrap();
	assert_eq!(merged.len(), 3);
	assert_eq!(
		merged[0]
			.dependencies
			.iter()
			.map(|pin| pin.resource.as_str())
			.collect::<Vec<_>>(),
		["first", "second"]
	);
	assert!(merged[0].mandatory);
	let mut changed = merged[0].clone();
	changed.content = json!("changed behind same revision");
	assert!(deduplicate(vec![merged[0].clone(), changed]).is_err());
}
#[test]
fn final_fit_prioritizes_mandatory_receipt_and_cannot_silently_drop_it() {
	let optional = candidate("optional", "1", "optional", false);
	let mandatory = candidate("home", "1", "home", true);
	let fitted = fit_with(vec![optional.clone(), mandatory.clone()], |items| {
		Ok(items.len() <= 1)
	})
	.unwrap();
	assert_eq!(fitted.contributions[0].identity, mandatory.identity);
	assert_eq!(fitted.deferred, [optional.identity]);
	assert!(fit_with(vec![mandatory], |_| Ok(false)).is_err());
}
