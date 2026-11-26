use super::*;
use crate::transactions::Participant;
use rstest::{fixture, rstest};

const ALPHA: &str = "aidash://alpha/airy-hawk-2lu4as22";
const BETA: &str = "aidash://beta/brave-otter-k3m9q2xa";
const SEED: &str = "5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e";

#[fixture]
fn manifest() -> Manifest {
	Manifest {
		id: Uuid::from_u128(7),
		coordinator: ALPHA.into(),
		isolation: Isolation::Serializable,
		deadline: "2030-01-01T00:00:00Z".parse().unwrap(),
		participants: vec![
			Participant {
				node_id: ALPHA.into(),
				mutations: vec![Mutation::ThreadTitle {
					thread_id: Uuid::from_u128(1),
					expected_revision: 2,
					title: "alpha-secret".into(),
				}],
			},
			Participant {
				node_id: BETA.into(),
				mutations: vec![Mutation::FinishRun {
					run_id: Uuid::from_u128(3),
					task_id: Uuid::from_u128(4),
					expected_revision: 5,
				}],
			},
		],
	}
}

#[rstest]
fn every_participant_agrees_on_one_digest_without_the_other_parts(manifest: Manifest) {
	let alpha = manifest.share(SEED, ALPHA).unwrap();
	let beta = manifest.share(SEED, BETA).unwrap();
	alpha.validate().unwrap();
	beta.validate().unwrap();
	assert_eq!(alpha.digest().unwrap(), beta.digest().unwrap());
	assert_eq!(
		alpha.digest().unwrap(),
		manifest.sealed_digest(SEED).unwrap()
	);
	assert_eq!(alpha.recipients(), vec![ALPHA.to_owned(), BETA.to_owned()]);
	// Beta's share carries neither Alpha's value nor Alpha's random value.
	let encoded = serde_json::to_string(&beta).unwrap();
	assert!(!encoded.contains("alpha-secret"));
	assert!(!encoded.contains(&alpha.part.nonce));
	assert_ne!(alpha.part.nonce, beta.part.nonce);
}

#[rstest]
fn a_seal_reveals_nothing_without_the_random_value(manifest: Manifest) {
	let seals = manifest.seals(SEED).unwrap();
	let guessed = Part {
		node_id: ALPHA.into(),
		nonce: "0".repeat(64),
		mutations: manifest.participants[0].mutations.clone(),
	};
	assert_ne!(guessed.seal().unwrap().hash, seals[0].hash);
	assert_ne!(
		manifest.sealed_digest(SEED).unwrap(),
		manifest.sealed_digest(&"1".repeat(64)).unwrap()
	);
}

#[rstest]
fn claims_name_task_completions_and_run_finalizations_only(manifest: Manifest) {
	let share = manifest.share(SEED, ALPHA).unwrap();
	assert!(share.seals[0].claims.is_empty());
	assert!(share.claims(BETA, &claim("finish_run", Uuid::from_u128(4))));
	assert!(!share.claims(BETA, &claim("complete_task", Uuid::from_u128(4))));
	assert!(!share.claims(ALPHA, &claim("finish_run", Uuid::from_u128(4))));
}

#[rstest]
#[case::changed_part("part")]
#[case::forged_claim("claim")]
#[case::foreign_part("foreign")]
#[case::short_nonce("nonce")]
#[case::changed_seal("seal")]
fn a_share_whose_part_does_not_match_its_own_seal_is_refused(
	manifest: Manifest,
	#[case] change: &str,
) {
	let mut share = manifest.share(SEED, ALPHA).unwrap();
	let digest = share.digest().unwrap();
	match change {
		"part" => {
			share.part.mutations = vec![Mutation::ThreadTitle {
				thread_id: Uuid::from_u128(1),
				expected_revision: 2,
				title: "another".into(),
			}]
		}
		"claim" => share.seals[0]
			.claims
			.push(claim("complete_task", Uuid::from_u128(9))),
		"foreign" => share.part.node_id = "aidash://gamma/calm-heron-9x8y7z6w".into(),
		"nonce" => share.part.nonce = "00".into(),
		"seal" => share.seals[0].hash = "f".repeat(64),
		_ => unreachable!(),
	}
	assert!(share.validate().is_err(), "{change}");
	if change == "claim" || change == "seal" {
		// A changed seal also changes what everyone agrees on.
		assert_ne!(share.digest().unwrap(), digest);
	}
}

#[rstest]
fn a_share_requires_its_coordinator_among_sorted_participants(manifest: Manifest) {
	let mut share = manifest.share(SEED, BETA).unwrap();
	share.seals.swap(0, 1);
	assert!(share.validate().is_err());
	let mut share = manifest.share(SEED, BETA).unwrap();
	share.seals.remove(0);
	assert!(share.validate().is_err());
}

#[rstest]
fn a_share_for_a_node_outside_the_manifest_is_forbidden(manifest: Manifest) {
	assert_eq!(
		manifest.share(SEED, "aidash://gamma/calm-heron-9x8y7z6w"),
		Err(Error::Invalid(
			"the Node takes no part in this transaction".into()
		))
	);
}
