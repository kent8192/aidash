use super::*;
use crate::memory::{
	Bank,
	recovery::{Fence, Ledger},
};
use uuid::Uuid;

fn seed() -> History {
	let mut ledger = Ledger::new("home".into());
	ledger.epoch = Uuid::from_u128(1);
	History::new(ledger, "a".repeat(64)).unwrap()
}
fn floor(revision: i64, deleted: bool, digest: &str) -> Floor {
	Floor {
		id: Uuid::from_u128(10),
		fence: Fence {
			bank: Bank {
				home: "home".into(),
				tenant: "tenant".into(),
				workspace: Uuid::from_u128(20),
				participant: None,
			},
			revision,
			deleted,
			digest: digest.into(),
		},
		visible: !deleted,
	}
}
fn publish(history: &mut History, units: Vec<Floor>, recovering: bool) {
	let record = history
		.next(Uuid::new_v4(), Change::Floors { units, recovering })
		.unwrap();
	history.apply(&record).unwrap();
}

#[test]
fn durable_history_keeps_failed_commit_floors_and_permanent_deletions() {
	let mut history = seed();
	publish(&mut history, vec![floor(1, false, "original")], false);
	// There is deliberately no SQL outcome in this history: independent
	// durability must survive a failed business commit.
	publish(&mut history, vec![floor(2, true, "deleted")], false);
	let revive = history
		.next(
			Uuid::new_v4(),
			Change::Floors {
				units: vec![floor(3, false, "revived")],
				recovering: false,
			},
		)
		.unwrap();
	assert!(history.apply(&revive).is_err());
	let saved = &history.ledger.units[&Uuid::from_u128(10)];
	assert_eq!(
		(saved.revision, saved.deleted, saved.digest.as_str()),
		(2, true, "deleted")
	);
	assert_eq!(history.head.sequence, 2);
}

#[test]
fn history_rejects_gaps_epoch_changes_and_same_revision_visible_rewrites() {
	let mut history = seed();
	publish(&mut history, vec![floor(1, false, "original")], false);
	let valid = history
		.next(
			Uuid::new_v4(),
			Change::Floors {
				units: vec![floor(2, false, "corrected")],
				recovering: false,
			},
		)
		.unwrap();
	let mut gap = valid.clone();
	gap.sequence += 1;
	assert!(history.apply(&gap).is_err());
	let mut epoch = valid;
	epoch.epoch = Uuid::new_v4();
	assert!(history.apply(&epoch).is_err());
	let rewrite = history
		.next(
			Uuid::new_v4(),
			Change::Floors {
				units: vec![floor(1, false, "different")],
				recovering: false,
			},
		)
		.unwrap();
	assert!(history.apply(&rewrite).is_err());
	assert_eq!(history.head.sequence, 1);
	assert_eq!(
		history.ledger.units[&Uuid::from_u128(10)].digest,
		"original"
	);
}

#[test]
fn recovery_barrier_fences_old_writers_until_a_new_generation_resumes() {
	let mut history = seed();
	let barrier = history.next(Uuid::new_v4(), Change::Barrier).unwrap();
	history.apply(&barrier).unwrap();
	assert!(history.head.recovering);
	assert_eq!(history.head.generation, 1);
	let old = history
		.next(
			Uuid::new_v4(),
			Change::Floors {
				units: vec![floor(1, false, "body")],
				recovering: false,
			},
		)
		.unwrap();
	assert!(history.apply(&old).is_err());
	let mut wrong_generation = history
		.next(
			Uuid::new_v4(),
			Change::Floors {
				units: vec![floor(1, false, "body")],
				recovering: true,
			},
		)
		.unwrap();
	wrong_generation.generation = 0;
	assert!(history.apply(&wrong_generation).is_err());
	publish(&mut history, vec![floor(1, false, "body")], true);
	let resume = history
		.next(
			Uuid::new_v4(),
			Change::Resume {
				evidence: "verified-all-banks".into(),
			},
		)
		.unwrap();
	history.apply(&resume).unwrap();
	assert!(!history.head.recovering);
	assert_eq!(history.head.generation, 2);
	publish(&mut history, vec![floor(2, false, "corrected")], false);
	assert_eq!(history.ledger.units[&Uuid::from_u128(10)].revision, 2);
}

#[test]
fn invalid_batch_does_not_partially_advance_history() {
	let mut history = seed();
	publish(&mut history, vec![floor(2, false, "original")], false);
	let record = history
		.next(
			Uuid::new_v4(),
			Change::Floors {
				units: vec![floor(3, false, "new"), floor(1, false, "old")],
				recovering: false,
			},
		)
		.unwrap();
	assert!(history.apply(&record).is_err());
	assert_eq!(history.head.sequence, 1);
	assert_eq!(history.ledger.units[&Uuid::from_u128(10)].revision, 2);
}
