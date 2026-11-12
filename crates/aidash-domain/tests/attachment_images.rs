//! Thread Attachment images in the semantic index (#622 part 2).
use aidash_domain::semantic::{
	EmbeddingInput,
	indexing::{
		MAX_IMAGE_DESCRIPTION_CHARS, attachment_image_key, content_digest, image_digest,
		validate_image_description,
	},
};

#[test]
fn an_image_without_a_description_keeps_its_bytes_digest() {
	// Arrange
	let bytes = "ab".repeat(32);

	// Act
	let digest = image_digest(&bytes, None);

	// Assert
	assert_eq!(digest, bytes);
}

#[test]
fn editing_the_description_changes_the_point_digest() {
	// Arrange
	let bytes = "ab".repeat(32);

	// Act
	let first = image_digest(&bytes, Some("A red bicycle"));
	let second = image_digest(&bytes, Some("A blue bicycle"));

	// Assert
	assert_ne!(first, bytes);
	assert_ne!(first, second);
	assert_eq!(first, content_digest(&format!("{bytes}\nA red bicycle")));
}

#[test]
fn descriptions_are_bounded_prose() {
	assert!(validate_image_description("A chart\nwith two lines").is_ok());
	assert!(validate_image_description("   ").is_err());
	assert!(validate_image_description("bell\u{7}").is_err());
	assert!(validate_image_description(&"x".repeat(MAX_IMAGE_DESCRIPTION_CHARS)).is_ok());
	assert!(validate_image_description(&"x".repeat(MAX_IMAGE_DESCRIPTION_CHARS + 1)).is_err());
	// Counted in characters: a full-length Japanese description fits.
	assert!(validate_image_description(&"画".repeat(MAX_IMAGE_DESCRIPTION_CHARS)).is_ok());
}

#[test]
fn a_described_image_reserves_its_description_with_the_image() {
	// Arrange
	let bytes = vec![0u8; 257];
	let plain = EmbeddingInput::Image {
		media_type: "image/png",
		bytes: &bytes,
	};
	let described = EmbeddingInput::DescribedImage {
		text: "abcd",
		media_type: "image/png",
		bytes: &bytes,
	};

	// Act
	let plain = plain.reservation_amount().unwrap();
	let described = described.reservation_amount().unwrap();

	// Assert
	assert_eq!(described, plain + 4);
}

#[test]
fn one_attachment_has_one_entry_key() {
	let id = uuid::Uuid::nil();
	assert_eq!(
		attachment_image_key(id),
		"attachment-image:00000000-0000-0000-0000-000000000000"
	);
}
