use super::*;
use crate::transactions::{Isolation, Participant};
use rstest::{fixture, rstest};

#[fixture]
fn manifest() -> Manifest {
	Manifest {
		id: Uuid::from_u128(1),
		coordinator: "aidash://home".into(),
		isolation: Isolation::Serializable,
		deadline: "2030-01-01T00:00:00Z".parse().unwrap(),
		participants: vec![Participant {
			node_id: "aidash://home".into(),
			mutations: vec![],
		}],
	}
}

fn state(manifest: &Manifest, decision: Option<&str>, visible: bool) -> Status {
	Status {
		id: manifest.id,
		digest: manifest.digest().unwrap(),
		manifest: json!(manifest),
		decision: decision.map(str::to_owned),
		visible,
		complete: false,
		last_error: None,
		created_at: "2030-01-01T00:00:00Z".parse().unwrap(),
	}
}

fn votes(phases: &[&str]) -> Vec<Vote> {
	phases
		.iter()
		.enumerate()
		.map(|(index, phase)| Vote {
			node_id: format!("aidash://node-{index}"),
			phase: (*phase).into(),
		})
		.collect()
}

#[rstest]
#[case::reserve_before_prepare(None, false, & ["RESERVED", "PENDING"], Some((1, ParticipantOperation::Reserve)))]
#[case::prepare_after_reservation(None, false, & ["PREPARED", "RESERVED"], Some((1, ParticipantOperation::Prepare)))]
#[case::prepared(None, false, & ["PREPARED", "PREPARED"], None)]
#[case::abort(Some("ABORT"), false, & ["ABORTED", "PREPARED"], Some((1, ParticipantOperation::Finish)))]
#[case::applied_barrier(Some("COMMIT"), false, & ["APPLIED", "COMMITTED", "PREPARED"], Some((2, ParticipantOperation::Finish)))]
#[case::ready_to_publish(Some("COMMIT"), false, & ["APPLIED", "COMMITTED"], None)]
#[case::finalize_after_visibility(Some("COMMIT"), true, & ["COMMITTED", "APPLIED"], Some((1, ParticipantOperation::Finish)))]
#[case::finalized(Some("COMMIT"), true, & ["COMMITTED", "COMMITTED"], None)]
fn vote_order_preserves_reservation_and_visibility_barriers(
	manifest: Manifest,
	#[case] decision: Option<&str>,
	#[case] visible: bool,
	#[case] phases: &[&str],
	#[case] expected: Option<(usize, ParticipantOperation)>,
) {
	let state = state(&manifest, decision, visible);
	let votes = votes(phases);
	let selected = next_participant(&state, &votes);
	assert_eq!(
		selected.map(|(vote, operation)| (vote.node_id.as_str(), operation)),
		expected.map(|(index, operation)| (votes[index].node_id.as_str(), operation)),
	);
}

#[rstest]
#[case::reserve(ParticipantOperation::Reserve, None, false, "RESERVED")]
#[case::prepare(ParticipantOperation::Prepare, None, false, "PREPARED")]
#[case::abort(ParticipantOperation::Finish, Some("ABORT"), false, "ABORTED")]
#[case::apply(ParticipantOperation::Finish, Some("COMMIT"), false, "APPLIED")]
#[case::finalize(ParticipantOperation::Finish, Some("COMMIT"), true, "COMMITTED")]
fn acknowledgements_require_the_exact_protocol_phase(
	manifest: Manifest,
	#[case] operation: ParticipantOperation,
	#[case] decision: Option<&str>,
	#[case] visible: bool,
	#[case] expected: &str,
) {
	let state = state(&manifest, decision, visible);
	let mut response = LocalStatus {
		id: manifest.id,
		coordinator: manifest.coordinator.clone(),
		digest: state.digest.clone(),
		manifest: state.manifest.clone(),
		phase: expected.into(),
		updated_at: state.created_at,
	};
	validate_acknowledgement(&manifest, &state, operation, &response).unwrap();
	response.phase = "PENDING".into();
	assert!(matches!(
		validate_acknowledgement(&manifest, &state, operation, &response),
		Err(Error::Conflict(message)) if message == "participant returned an unexpected phase"
	));
}

#[rstest]
#[case::id("id")]
#[case::coordinator("coordinator")]
#[case::digest("digest")]
#[case::manifest("manifest")]
fn acknowledgements_are_bound_to_all_immutable_manifest_facts(
	manifest: Manifest,
	#[case] changed: &str,
) {
	let state = state(&manifest, None, false);
	let mut response = LocalStatus {
		id: manifest.id,
		coordinator: manifest.coordinator.clone(),
		digest: state.digest.clone(),
		manifest: state.manifest.clone(),
		phase: "RESERVED".into(),
		updated_at: state.created_at,
	};
	match changed {
		"id" => response.id = Uuid::from_u128(2),
		"coordinator" => response.coordinator = "aidash://other".into(),
		"digest" => response.digest = "other".into(),
		"manifest" => response.manifest["participants"] = json!([]),
		_ => unreachable!(),
	}
	assert!(matches!(
		validate_acknowledgement(&manifest, &state, ParticipantOperation::Reserve, &response),
		Err(Error::Conflict(message)) if message == "participant acknowledged another manifest"
	));
}

#[rstest]
#[case::undecided(None, false, true)]
#[case::commit(Some("COMMIT"), false, true)]
#[case::visible_commit(Some("COMMIT"), true, true)]
#[case::abort(Some("ABORT"), false, true)]
#[case::unknown(Some("OTHER"), false, false)]
#[case::visible_undecided(None, true, false)]
#[case::visible_abort(Some("ABORT"), true, false)]
fn decision_proofs_preserve_monotonic_visibility(
	manifest: Manifest,
	#[case] decision: Option<&str>,
	#[case] visible: bool,
	#[case] expected: bool,
) {
	let proof = state(&manifest, decision, visible);
	assert_eq!(decision_matches(&manifest, &proof.digest, &proof), expected);
}

#[rstest]
#[case::id("id")]
#[case::digest("digest")]
#[case::manifest("manifest")]
fn decision_proofs_cannot_substitute_another_manifest(manifest: Manifest, #[case] changed: &str) {
	let mut proof = state(&manifest, Some("COMMIT"), true);
	let digest = proof.digest.clone();
	match changed {
		"id" => proof.id = Uuid::from_u128(2),
		"digest" => proof.digest = "other".into(),
		"manifest" => proof.manifest["deadline"] = json!("2031-01-01T00:00:00Z"),
		_ => unreachable!(),
	}
	assert_eq!(decision_matches(&manifest, &digest, &proof), false);
}

#[rstest]
#[case::decide(None, false, & ["PREPARED"], "COMMIT", "")]
#[case::publish(Some("COMMIT"), false, & ["APPLIED"], "VISIBLE", "every participant durably applied commit")]
#[case::complete_commit(Some("COMMIT"), true, & ["COMMITTED"], "COMPLETE", "every participant finalized")]
#[case::complete_abort(Some("ABORT"), false, & ["ABORTED"], "COMPLETE", "every participant finalized")]
fn durable_transition_keeps_the_original_audit_detail(
	manifest: Manifest,
	#[case] decision: Option<&str>,
	#[case] visible: bool,
	#[case] phases: &[&str],
	#[case] expected_phase: &str,
	#[case] expected_detail: &str,
) {
	let state = state(&manifest, decision, visible);
	let (change, detail) = next_transition(&state, &votes(phases)).unwrap();
	assert_eq!((change.phase(), detail), (expected_phase, expected_detail));
}

#[rstest]
fn commit_requires_every_prepared_vote(manifest: Manifest) {
	let state = state(&manifest, None, false);
	assert!(matches!(
		next_transition(&state, &votes(&["PREPARED", "ABORTED"])),
		Err(Error::Conflict(message)) if message == "commit requires every prepared vote"
	));
}

#[rstest]
#[case::reserved("RESERVED", true, false)]
#[case::prepared("PREPARED", true, true)]
#[case::applied("APPLIED", false, true)]
#[case::committed("COMMITTED", false, false)]
#[case::aborted("ABORTED", true, false)]
fn participant_phase_preserves_abort_and_commit_preconditions(
	manifest: Manifest,
	#[case] phase: &str,
	#[case] can_abort: bool,
	#[case] can_commit: bool,
) {
	let state = state(&manifest, None, false);
	let existing = LocalStatus {
		id: manifest.id,
		coordinator: manifest.coordinator.clone(),
		digest: state.digest,
		manifest: state.manifest,
		phase: phase.into(),
		updated_at: state.created_at,
	};
	assert_eq!(
		(abortable(&existing), commit_ready(&existing)),
		(can_abort, can_commit)
	);
	assert_eq!(
		participant_matches(&manifest, &manifest.digest().unwrap(), &existing),
		true
	);
}
