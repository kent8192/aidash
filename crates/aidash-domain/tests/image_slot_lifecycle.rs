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
