use aidash_domain::semantic::indexing::{ImageChange, ImageSlots};
use uuid::Uuid;

fn publish(id: u128) -> ImageChange {
	ImageChange::Published {
		file_id: Uuid::from_u128(id),
		path: format!("{id}.png"),
		media_type: "image/png".into(),
		size: 4,
	}
}

#[test]
fn removal_releases_a_slot_without_readmitting_an_older_limited_file() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(area, 1, 1, &[publish(10), publish(11), publish(12)], 2);
	assert!(slots.assigned(area, Uuid::from_u128(10)));
	assert!(slots.assigned(area, Uuid::from_u128(11)));
	assert!(!slots.assigned(area, Uuid::from_u128(12)));
	slots.apply(
		area,
		1,
		2,
		&[ImageChange::Removed {
			file_id: Uuid::from_u128(10),
			path: "10.png".into(),
		}],
		2,
	);
	assert!(!slots.assigned(area, Uuid::from_u128(12)));
	slots.apply(area, 1, 3, &[publish(13)], 2);
	assert!(slots.assigned(area, Uuid::from_u128(13)));
	assert!(!slots.assigned(area, Uuid::from_u128(12)));
}

#[test]
fn rebuild_rechecks_surviving_images_in_original_publication_order() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(area, 1, 1, &[publish(10), publish(11), publish(12)], 2);
	slots.apply(
		area,
		1,
		2,
		&[ImageChange::Removed {
			file_id: Uuid::from_u128(10),
			path: "10.png".into(),
		}],
		2,
	);
	slots.apply(area, 1, 3, &[publish(13)], 2);
	slots.rebuild(2);
	assert!(slots.assigned(area, Uuid::from_u128(11)));
	assert!(slots.assigned(area, Uuid::from_u128(12)));
	assert!(!slots.assigned(area, Uuid::from_u128(13)));
}

#[test]
fn replacing_an_image_releases_its_previous_slot_before_admitting_its_new_version() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(area, 1, 1, &[publish(10)], 1);
	slots.apply(
		area,
		1,
		2,
		&[
			ImageChange::Removed {
				file_id: Uuid::from_u128(10),
				path: "10.png".into(),
			},
			ImageChange::Published {
				file_id: Uuid::from_u128(11),
				path: "10.png".into(),
				media_type: "image/png".into(),
				size: 4,
			},
		],
		1,
	);
	assert!(!slots.assigned(area, Uuid::from_u128(10)));
	assert!(slots.assigned(area, Uuid::from_u128(11)));
}

#[test]
fn legacy_baseline_rebuild_uses_area_then_manifest_order() {
	let first = Uuid::from_u128(1);
	let second = Uuid::from_u128(2);
	let mut slots = ImageSlots::default();
	slots.apply(second, 1, 0, &[publish(30)], 2);
	slots.apply(first, 1, 0, &[publish(10), publish(11)], 2);
	slots.rebuild(2);
	assert!(slots.assigned(first, Uuid::from_u128(10)));
	assert!(slots.assigned(first, Uuid::from_u128(11)));
	assert!(!slots.assigned(second, Uuid::from_u128(30)));
}

#[test]
fn queued_changes_after_a_rebuild_do_not_refill_a_deleted_slot() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(area, 1, 1, &[publish(10), publish(11), publish(12)], 2);
	// Rebuild was requested after publication 2, before publication 3.
	slots.request_rebuild(Some(2));
	slots.apply(area, 1, 2, &[publish(13)], 2);
	assert!(slots.finish_publication(2, 2));
	slots.apply(
		area,
		1,
		3,
		&[ImageChange::Removed {
			file_id: Uuid::from_u128(10),
			path: "10.png".into(),
		}],
		2,
	);
	assert!(!slots.finish_publication(3, 2));
	assert!(!slots.assigned(area, Uuid::from_u128(12)));
	assert!(!slots.assigned(area, Uuid::from_u128(13)));
}

#[test]
fn a_legacy_candidate_waits_without_blocking_later_images_and_retains_manifest_order() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(
		area,
		1,
		-1,
		&[ImageChange::LegacyCandidate {
			file_id: Uuid::from_u128(10),
			path: "unknown".into(),
			position: 0,
		}],
		1,
	);
	slots.apply(area, 1, 1, &[publish(11)], 1);
	assert!(slots.assigned(area, Uuid::from_u128(11)));
	slots.apply(
		area,
		1,
		-2,
		&[ImageChange::LegacyResolved {
			file_id: Uuid::from_u128(10),
			path: "unknown".into(),
			position: 0,
			media_type: Some("image/png".into()),
			size: 4,
		}],
		1,
	);
	assert!(slots.assigned(area, Uuid::from_u128(10)));
	assert!(!slots.assigned(area, Uuid::from_u128(11)));
}

#[test]
fn removed_legacy_candidate_cannot_be_resurrected_by_a_late_probe() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(
		area,
		1,
		-1,
		&[ImageChange::LegacyCandidate {
			file_id: Uuid::from_u128(10),
			path: "unknown".into(),
			position: 0,
		}],
		1,
	);
	slots.apply(
		area,
		1,
		1,
		&[ImageChange::Removed {
			file_id: Uuid::from_u128(10),
			path: "unknown".into(),
		}],
		1,
	);
	slots.apply(
		area,
		1,
		-2,
		&[ImageChange::LegacyResolved {
			file_id: Uuid::from_u128(10),
			path: "unknown".into(),
			position: 0,
			media_type: Some("image/png".into()),
			size: 4,
		}],
		1,
	);
	assert!(slots.entries.is_empty());
	assert!(slots.legacy_candidates.is_empty());
}

#[test]
fn a_legacy_non_image_releases_its_candidate_without_using_a_slot() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(
		area,
		1,
		-1,
		&[ImageChange::LegacyCandidate {
			file_id: Uuid::from_u128(10),
			path: "unknown".into(),
			position: 0,
		}],
		1,
	);
	slots.apply(
		area,
		1,
		-2,
		&[ImageChange::LegacyResolved {
			file_id: Uuid::from_u128(10),
			path: "unknown".into(),
			position: 0,
			media_type: None,
			size: 4,
		}],
		1,
	);
	assert!(slots.entries.is_empty());
	assert!(slots.legacy_candidates.is_empty());
}

#[test]
fn a_late_legacy_probe_does_not_fill_a_slot_freed_after_its_initial_limit() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(area, 1, 0, &[publish(10)], 1);
	slots.apply(
		area,
		1,
		-1,
		&[ImageChange::LegacyCandidate {
			file_id: Uuid::from_u128(11),
			path: "unknown".into(),
			position: 1,
		}],
		1,
	);
	slots.apply(
		area,
		1,
		1,
		&[ImageChange::Removed {
			file_id: Uuid::from_u128(10),
			path: "10.png".into(),
		}],
		1,
	);
	slots.apply(
		area,
		1,
		-2,
		&[ImageChange::LegacyResolved {
			file_id: Uuid::from_u128(11),
			path: "unknown".into(),
			position: 1,
			media_type: Some("image/png".into()),
			size: 4,
		}],
		1,
	);
	assert!(!slots.assigned(area, Uuid::from_u128(11)));
}

#[test]
fn rebuilding_reconsiders_an_unresolved_legacy_candidate() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(area, 1, 0, &[publish(10)], 1);
	slots.apply(
		area,
		1,
		-1,
		&[ImageChange::LegacyCandidate {
			file_id: Uuid::from_u128(11),
			path: "unknown".into(),
			position: 1,
		}],
		1,
	);
	slots.apply(
		area,
		1,
		1,
		&[ImageChange::Removed {
			file_id: Uuid::from_u128(10),
			path: "10.png".into(),
		}],
		1,
	);
	slots.rebuild(1);
	slots.apply(
		area,
		1,
		-2,
		&[ImageChange::LegacyResolved {
			file_id: Uuid::from_u128(11),
			path: "unknown".into(),
			position: 1,
			media_type: Some("image/png".into()),
			size: 4,
		}],
		1,
	);
	assert!(slots.assigned(area, Uuid::from_u128(11)));
}

#[test]
fn resolving_an_earlier_candidate_marks_later_candidates_as_limited() {
	let area = Uuid::from_u128(1);
	let mut slots = ImageSlots::default();
	slots.apply(
		area,
		1,
		-1,
		&[
			ImageChange::LegacyCandidate {
				file_id: Uuid::from_u128(10),
				path: "first".into(),
				position: 0,
			},
			ImageChange::LegacyCandidate {
				file_id: Uuid::from_u128(11),
				path: "second".into(),
				position: 1,
			},
		],
		1,
	);
	slots.apply(
		area,
		1,
		-2,
		&[ImageChange::LegacyResolved {
			file_id: Uuid::from_u128(10),
			path: "first".into(),
			position: 0,
			media_type: Some("image/png".into()),
			size: 4,
		}],
		1,
	);
	slots.apply(
		area,
		1,
		1,
		&[ImageChange::Removed {
			file_id: Uuid::from_u128(10),
			path: "first".into(),
		}],
		1,
	);
	slots.apply(
		area,
		1,
		-3,
		&[ImageChange::LegacyResolved {
			file_id: Uuid::from_u128(11),
			path: "second".into(),
			position: 1,
			media_type: Some("image/png".into()),
			size: 4,
		}],
		1,
	);
	assert!(!slots.assigned(area, Uuid::from_u128(11)));
}
