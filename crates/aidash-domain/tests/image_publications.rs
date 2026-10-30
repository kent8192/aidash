use aidash_domain::{
	capabilities::operations::{FileScope, MountedFile},
	semantic::indexing::{ImageChange, image_changes},
};
use serde_json::json;
use uuid::Uuid;

fn file(id: u128, path: &str, media_type: &str, scope: FileScope) -> MountedFile {
	MountedFile {
		file_id: Uuid::from_u128(id),
		path: path.into(),
		digest: format!("{id:064x}"),
		size: id as u64,
		media_type: media_type.into(),
		scope,
		provenance: json!({}),
	}
}

#[test]
fn publication_retires_replaced_and_deleted_versions_before_ordered_additions() {
	let previous = vec![
		file(1, "replace.png", "image/png", FileScope::Working),
		file(2, "deleted.jpg", "image/jpeg", FileScope::Working),
		file(3, "unchanged.png", "image/png", FileScope::Working),
	];
	let current = vec![
		file(4, "replace.png", "image/png", FileScope::Working),
		file(3, "unchanged.png", "image/png", FileScope::Working),
		file(5, "new.webp", "image/webp", FileScope::Working),
	];
	let changes = image_changes(&previous, &current);
	assert_eq!(
		changes.iter().map(ImageChange::file_id).collect::<Vec<_>>(),
		[1, 2, 4, 5].map(Uuid::from_u128)
	);
	assert!(matches!(changes[0], ImageChange::Removed { .. }));
	assert!(matches!(changes[1], ImageChange::Removed { .. }));
	assert!(matches!(changes[2], ImageChange::Published { .. }));
	assert!(matches!(changes[3], ImageChange::Published { .. }));
}

#[test]
fn publication_ignores_other_scopes_and_does_not_readmit_unchanged_images() {
	let image = file(1, "image.png", "image/png", FileScope::Working);
	let previous = vec![
		image.clone(),
		file(2, "gone.png", "image/png", FileScope::Working),
	];
	let current = vec![
		image,
		file(3, "ref.png", "image/png", FileScope::References),
		file(4, "doc.txt", "text/plain", FileScope::Working),
	];
	let changes = image_changes(&previous, &current);
	assert_eq!(changes.len(), 1);
	assert!(
		matches!(&changes[0], ImageChange::Removed { file_id, .. } if *file_id == Uuid::from_u128(2))
	);
}

#[test]
fn received_images_are_current_files_of_the_receiving_working_area() {
	let current = vec![file(
		7,
		"transfer/image.png",
		"image/png",
		FileScope::Received,
	)];
	let changes = image_changes(&[], &current);
	assert!(
		matches!(&changes[..], [ImageChange::Published { file_id, .. }] if *file_id == Uuid::from_u128(7))
	);
}
