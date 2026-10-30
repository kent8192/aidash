use super::*;
#[test]
fn reference_matches_are_ranked_and_limited_across_sources() {
	use crate::registry::bindings::QualifiedRef;
	let source = |id: &str| QualifiedRef {
		registry_deployment: "node".into(),
		id: id.into(),
		version: "1.0.0".into(),
	};
	let item = |document_index, score| ImageMatch {
		document_index,
		score,
		name: "image".into(),
		media_type: "image/png".into(),
		size: 1,
		digest: "digest".into(),
	};
	let ranked = rank_matches(
		vec![
			(source("b"), item(0, 0.9)),
			(source("a"), item(2, 0.9)),
			(source("a"), item(1, 0.9)),
			(source("a"), item(0, 0.5)),
		],
		2,
	);
	assert_eq!(
		ranked
			.iter()
			.map(|(source, item)| (source.id.as_str(), item.document_index))
			.collect::<Vec<_>>(),
		vec![("a", 1), ("a", 2)]
	);
}

#[test]
fn reference_matches_use_only_tokens_remaining_after_workspace_results() {
	let first = serde_json::json!({"name":"first.png"});
	let second = serde_json::json!({"name":"second.png"});
	let one_cost = crate::context::estimated_tokens(
		&serde_json::json!({"reference_matches":[first.clone()],"reference_truncated":false})
			.to_string(),
	);
	let (kept, truncated) = fit_reference_matches(vec![first.clone(), second], 100, 100 + one_cost);
	assert_eq!(kept, vec![first]);
	assert!(truncated);
	assert!(
		fit_reference_matches(vec![serde_json::json!({"name":"x"})], 100, 100)
			.0
			.is_empty()
	);
}
#[rstest::rstest]
fn documents_are_bounded_and_nonempty() {
	let mut docs = vec![ReferenceDocument {
		name: "personal.pdf".into(),
		media_type: "application/pdf".into(),
		text: "Reference data".into(),
		data_base64: None,
		image_digest: None,
		image_size: None,
	}];
	assert!(validate(&docs).is_ok());
	docs[0].name = "invalid\0name".into();
	assert!(validate(&docs).unwrap_err().to_string().contains("NUL"));
	docs[0].name = "personal.pdf".into();
	docs[0].text.clear();
	assert!(validate(&docs).is_err());
	docs[0].text = "x".repeat(65537);
	assert!(validate(&docs).is_err());
	assert!(validate(&[]).is_err());
}

#[test]
fn reference_images_require_valid_original_bytes_and_no_extracted_text() {
	use base64::{Engine as _, engine::general_purpose::STANDARD};
	let png = b"\x89PNG\r\n\x1a\nreference-image";
	let document: ReferenceDocument = serde_json::from_value(serde_json::json!({
		"name":"chart.png", "media_type":"image/png", "data_base64":STANDARD.encode(png)
	}))
	.unwrap();
	assert_eq!(
		image_bytes(&document).unwrap().as_deref(),
		Some(png.as_slice())
	);
	assert!(validate(std::slice::from_ref(&document)).is_ok());
	let mut spoofed = document.clone();
	spoofed.media_type = "image/jpeg".into();
	assert!(image_bytes(&spoofed).is_err());
	let mut mixed = document.clone();
	mixed.text = "not an image description".into();
	assert!(image_bytes(&mixed).is_err());
	let mut invalid = document;
	invalid.data_base64 = Some("%%%".into());
	assert!(image_bytes(&invalid).is_err());
	invalid.data_base64 = Some("A".repeat(MAX_IMAGE_BYTES.div_ceil(3) * 4 + 1));
	assert!(image_bytes(&invalid).is_err());
}

#[test]
fn reference_set_validates_every_image_before_it_can_be_embedded() {
	use base64::{Engine as _, engine::general_purpose::STANDARD};
	let png = STANDARD.encode(b"\x89PNG\r\n\x1a\nimage");
	let mut docs: Vec<ReferenceDocument> = serde_json::from_value(serde_json::json!([
		{"name":"good.png","media_type":"image/png","data_base64":png},
		{"name":"bad.png","media_type":"image/png","data_base64":"%%%"}
	]))
	.unwrap();
	assert!(validated_images(&docs).is_err());
	docs.pop();
	let images = validated_images(&docs).unwrap();
	assert_eq!(images.len(), 1);
	assert_eq!(images[0].document_index, 0);
}

#[test]
fn automatic_reference_context_lists_images_by_name_without_their_bytes() {
	let documents: Vec<ReferenceDocument> = serde_json::from_value(serde_json::json!([
		{"name":"notes.txt","media_type":"text/plain","text":"Evidence"},
		{"name":"photo.png","media_type":"image/png","data_base64":"c2VjcmV0"}
	]))
	.unwrap();
	let context = visible_documents(&documents).unwrap();
	assert_eq!(context[0]["text"], "Evidence");
	assert_eq!(context[1], serde_json::json!({"name":"photo.png"}));
	assert!(!context.to_string().contains("c2VjcmV0"));
}

#[test]
fn private_image_collection_ranks_only_its_own_vectors_and_never_contains_bytes() {
	use base64::{Engine as _, engine::general_purpose::STANDARD};
	let png = b"\x89PNG\r\n\x1a\nprivate-image-bytes";
	let documents: Vec<ReferenceDocument> = serde_json::from_value(serde_json::json!([
		{"name":"first.png","media_type":"image/png","data_base64":STANDARD.encode(png)},
		{"name":"second.png","media_type":"image/png","data_base64":STANDARD.encode(png)}
	]))
	.unwrap();
	let embedding: crate::registry::Entry = serde_json::from_value(serde_json::json!({
		"id":"embed","version":"1.0.0","kind":"embedding","name":{"en":"Embed"},"description":{"en":"Embed"},
		"config":{"provider":"openai","endpoint":"https://example.com","model":"test","model_version":"1","dimensions":2,"inputs":["text","image"]}
	})).unwrap();
	let index = ImageCollection::new(
		"agent",
		"1.0.0",
		embedding,
		&documents,
		vec![vec![1.0, 0.0], vec![0.0, 1.0]],
	)
	.unwrap();
	let matches = index.search("agent", "1.0.0", &[0.0, 1.0], 2).unwrap();
	assert_eq!(
		matches
			.iter()
			.map(|item| item.name.as_str())
			.collect::<Vec<_>>(),
		["second.png", "first.png"]
	);
	assert!(index.search("other", "1.0.0", &[0.0, 1.0], 2).is_err());
	assert!(index.search("agent", "2.0.0", &[0.0, 1.0], 2).is_err());
	let (metadata, bytes) = canonical_documents(&documents).unwrap();
	assert!(metadata.iter().all(|doc| doc.data_base64.is_none()));
	assert_eq!(bytes[0]["data_base64"], STANDARD.encode(png));
	assert!(
		index
			.verify("agent", "1.0.0", &index.embedding, &metadata)
			.is_ok()
	);
	let mut changed = metadata;
	changed[0].name = "renamed.png".into();
	assert!(
		index
			.verify("agent", "1.0.0", &index.embedding, &changed)
			.is_err()
	);
	let encoded = serde_json::to_string(&index).unwrap();
	assert!(!encoded.contains("private-image-bytes"));
	assert!(!encoded.contains(&STANDARD.encode(png)));
}
