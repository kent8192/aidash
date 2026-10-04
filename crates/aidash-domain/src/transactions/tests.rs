use super::*;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn manifest() -> Manifest {
	serde_json::from_str(GOLDEN).unwrap()
}
fn check(manifest: &Manifest) -> crate::Result<()> {
	manifest.validate_with(|_| Ok(()))
}
#[rstest]
fn manifest_encoding_and_digest_preserve_the_original_wire_bytes(manifest: Manifest) {
	check(&manifest).unwrap();
	assert_eq!(serde_json::to_string(&manifest).unwrap(), GOLDEN);
	assert_eq!(manifest.digest().unwrap(), GOLDEN_DIGEST);
}
#[rstest]
fn local_participant_lookup_preserves_absence_as_a_distinct_authority_failure(manifest: Manifest) {
	assert_eq!(
		manifest.local("aidash://alpha").unwrap().node_id,
		"aidash://alpha"
	);
	assert_eq!(
		manifest.local("aidash://missing").unwrap_err().to_string(),
		"forbidden"
	);
}
#[rstest]
#[case::nil_id("nil")]
#[case::no_participants("empty")]
#[case::seventeen("seventeen")]
fn manifests_require_a_nonzero_identity_and_one_to_sixteen_participants(
	mut manifest: Manifest,
	#[case] invalid: &str,
) {
	match invalid {
		"nil" => manifest.id = Uuid::nil(),
		"empty" => manifest.participants.clear(),
		"seventeen" => manifest.participants = vec![manifest.participants[0].clone(); 17],
		_ => unreachable!(),
	}
	assert!(
		matches!(check(&manifest),Err(Error::Invalid(message)) if message=="transaction requires an ID and 1..16 participants")
	);
}
#[rstest]
#[case::duplicate("duplicate")]
#[case::unsorted("unsorted")]
#[case::too_many_mutations("mutations")]
fn ordering_unique_nodes_and_per_participant_mutation_capacity_remain_strict(
	mut manifest: Manifest,
	#[case] invalid: &str,
) {
	match invalid {
		"duplicate" => manifest.participants.push(manifest.participants[0].clone()),
		"unsorted" => {
			let mut second = manifest.participants[0].clone();
			second.node_id = "aidash://aardvark".into();
			manifest.participants.push(second);
		}
		"mutations" => {
			manifest.participants[0].mutations =
				vec![manifest.participants[0].mutations[0].clone(); 65]
		}
		_ => unreachable!(),
	}
	assert!(
		matches!(check(&manifest),Err(Error::Invalid(message)) if message=="participants must be unique, sorted by node ID, with at most 64 mutations each")
	);
}
#[rstest]
#[case::revision(-1,json!({}))]
#[case::array(0,json!([]))]
#[case::scalar(0,json!("state"))]
fn workspace_mutations_retain_revision_and_object_requirements(
	mut manifest: Manifest,
	#[case] revision: i64,
	#[case] state: Value,
) {
	manifest.participants[0].mutations = vec![Mutation::WorkspaceState {
		workspace_id: Uuid::from_u128(2),
		expected_revision: revision,
		state,
	}];
	assert!(
		matches!(check(&manifest),Err(Error::Invalid(message)) if message=="workspace mutation requires a revision and object state")
	);
}
#[rstest]
fn duplicate_mutation_identity_is_checked_per_participant_and_not_across_nodes(
	mut manifest: Manifest,
) {
	let duplicate = manifest.participants[0].mutations[0].clone();
	manifest.participants[0].mutations.push(duplicate);
	assert!(
		matches!(check(&manifest),Err(Error::Invalid(message)) if message=="a resource can be mutated only once per manifest")
	);
	manifest.participants[0].mutations.pop();
	let mut peer = manifest.participants[0].clone();
	peer.node_id = "aidash://beta".into();
	manifest.participants.push(peer);
	check(&manifest).unwrap();
}
#[rstest]
#[case::no_effects("effects")]
#[case::no_coordinator("coordinator")]
#[case::too_large("size")]
fn manifests_keep_the_original_completeness_and_size_gate(
	mut manifest: Manifest,
	#[case] invalid: &str,
) {
	match invalid {
		"effects" => manifest.participants[0].mutations.clear(),
		"coordinator" => manifest.coordinator = "aidash://beta".into(),
		"size" => {
			manifest.participants[0].mutations = vec![Mutation::WorkspaceState {
				workspace_id: Uuid::from_u128(2),
				expected_revision: 0,
				state: json!({"body":"a".repeat(524_288)}),
			}]
		}
		_ => unreachable!(),
	}
	assert!(
		matches!(check(&manifest),Err(Error::Invalid(message)) if message=="transaction needs mutations, its coordinator as a participant, and a manifest within 512 KiB")
	);
}
#[rstest]
fn injected_registry_validation_keeps_the_full_definition_and_its_error_identity(
	mut manifest: Manifest,
) {
	let entry:Entry=serde_json::from_value(json!({"id":"provider","version":"1.0.0","kind":"tool","name":{},"description":{},"config":{"saved":"definition"}})).unwrap();
	manifest.participants[0].mutations = vec![Mutation::RegistryRegister {
		entry: Box::new(entry.clone()),
	}];
	let mut calls = 0;
	let result: crate::Result<()> = manifest.validate_with(|saved| {
		calls += 1;
		assert_eq!(
			serde_json::to_value(saved).unwrap(),
			serde_json::to_value(&entry).unwrap()
		);
		Err(Error::Conflict("validator fault".into()))
	});
	assert_eq!(calls, 1);
	assert_eq!(result, Err(Error::Conflict("validator fault".into())));
}
#[rstest]
#[case::commit(CoordinatorDecision::Commit)]
#[case::abort(CoordinatorDecision::Abort)]
fn durable_coordinator_decisions_never_change_after_the_first_decision(
	#[case] decision: CoordinatorDecision,
) {
	let mut state = CoordinatorState {
		decision: None,
		visible: false,
		complete: false,
	};
	assert!(
		CoordinatorTransition::Decide(decision)
			.apply(&mut state)
			.unwrap()
	);
	let saved = state;
	for later in [CoordinatorDecision::Commit, CoordinatorDecision::Abort] {
		assert!(
			!CoordinatorTransition::Decide(later)
				.apply(&mut state)
				.unwrap()
		);
		assert_eq!(state, saved);
	}
}
#[rstest]
#[case::pending(None)]
#[case::aborted(Some(CoordinatorDecision::Abort))]
fn visibility_cannot_precede_a_committed_decision(#[case] decision: Option<CoordinatorDecision>) {
	let mut state = CoordinatorState {
		decision,
		visible: false,
		complete: false,
	};
	let before = state;
	assert!(
		matches!(CoordinatorTransition::Publish.apply(&mut state),Err(Error::Conflict(message)) if message=="only a committed transaction can become visible")
	);
	assert_eq!(state, before);
}
#[rstest]
fn committed_visibility_and_completion_are_each_monotonic_and_idempotent() {
	let mut state = CoordinatorState {
		decision: Some(CoordinatorDecision::Commit),
		visible: false,
		complete: false,
	};
	assert!(
		matches!(CoordinatorTransition::Complete.apply(&mut state),Err(Error::Conflict(message)) if message=="transaction must have a finalized decision")
	);
	assert!(!state.complete);
	assert!(CoordinatorTransition::Publish.apply(&mut state).unwrap());
	assert!(!CoordinatorTransition::Publish.apply(&mut state).unwrap());
	assert!(CoordinatorTransition::Complete.apply(&mut state).unwrap());
	assert!(!CoordinatorTransition::Complete.apply(&mut state).unwrap());
	assert_eq!(
		state,
		CoordinatorState {
			decision: Some(CoordinatorDecision::Commit),
			visible: true,
			complete: true
		}
	);
}
#[rstest]
fn aborted_decisions_can_complete_without_publishing_visible_effects() {
	let mut state = CoordinatorState {
		decision: Some(CoordinatorDecision::Abort),
		visible: false,
		complete: false,
	};
	assert!(CoordinatorTransition::Complete.apply(&mut state).unwrap());
	assert!(!CoordinatorTransition::Complete.apply(&mut state).unwrap());
	assert_eq!(
		state,
		CoordinatorState {
			decision: Some(CoordinatorDecision::Abort),
			visible: false,
			complete: true
		}
	);
	assert_eq!(
		CoordinatorTransition::Decide(CoordinatorDecision::Commit).phase(),
		"COMMIT"
	);
	assert_eq!(
		CoordinatorTransition::Decide(CoordinatorDecision::Abort).phase(),
		"ABORT"
	);
	assert_eq!(CoordinatorTransition::Publish.phase(), "VISIBLE");
	assert_eq!(CoordinatorTransition::Complete.phase(), "COMPLETE");
}
const GOLDEN: &str = r#"{"id":"00000000-0000-0000-0000-000000000001","coordinator":"aidash://alpha","isolation":"serializable","deadline":"2030-01-01T00:00:00Z","participants":[{"node_id":"aidash://alpha","mutations":[{"kind":"workspace_state","workspace_id":"00000000-0000-0000-0000-000000000002","expected_revision":0,"state":{"count":1}}]}]}"#;
const GOLDEN_DIGEST: &str = "650ef295266e86b21e72bbf41b7b6e3e9c4acca60ae248c5581e984c2105b9c2";
