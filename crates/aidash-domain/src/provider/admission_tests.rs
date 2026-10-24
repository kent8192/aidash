use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn request() -> ModelRequest {
	ModelRequest {
		instructions: "system".into(),
		context: json!({"text":"東京"}).into(),
		tools: vec![],
		max_output_tokens: 12,
		response_format: None,
		content_parts: vec![
			ContentPart::Text("abc".into()),
			ContentPart::Image {
				media_type: "image/png".into(),
				bytes: b"abc".to_vec(),
			},
			ContentPart::Audio {
				format: "wav".into(),
				bytes: b"abc".to_vec(),
			},
		],
		cache_scope: None,
		cache_breakpoints: false,
		disable_provider_transforms: false,
		estimate: Default::default(),
	}
}

#[rstest]
fn shadow_estimates_measure_the_same_request_without_changing_execution() {
	use super::estimate::{EstimateSnapshot, FRAMING_RESERVE};
	let request = ModelRequest {
		instructions: "private observation input ".repeat(64),
		max_output_tokens: 64,
		..Default::default()
	};
	let before = request.request_estimate();
	let digest = request.inference_digest();
	let snapshot = EstimateSnapshot::calibrated(7, 250_000, 20);
	let estimate = request.request_estimate_with_snapshot(&snapshot).unwrap();
	assert_eq!(estimate.byte_basis, before.byte_basis);
	assert_eq!(
		estimate.tokens,
		(before.byte_basis * 275_000).div_ceil(1_000_000) + 64 + FRAMING_RESERVE as u64
	);
	assert_eq!(estimate.estimator, "calibrated");
	assert_eq!(request.request_estimate(), before);
	assert_eq!(request.inference_digest(), digest);
	let record = usage::ShadowEstimate { snapshot, estimate };
	assert!(
		!serde_json::to_string(&record)
			.unwrap()
			.contains("private observation input")
	);
}

#[rstest]
fn shadow_tokenization_uses_the_supplied_encoding_and_preserves_unavailability() {
	use super::estimate::{EstimateSnapshot, TokenEncoding};
	let request = ModelRequest {
		instructions: "A moderately long request. ".repeat(100),
		max_output_tokens: 64,
		..Default::default()
	};
	let snapshot = EstimateSnapshot::tokenized(0, TokenEncoding::O200kBase, None);
	let shadow = request.request_estimate_with_snapshot(&snapshot);
	assert_eq!(shadow.is_some(), TokenEncoding::O200kBase.available());
	if let Some(shadow) = shadow {
		assert_eq!(shadow.estimator, "tokenized");
		assert!(shadow.tokens < request.request_estimate().tokens);
	}
	assert_eq!(request.estimate, EstimateSnapshot::conservative(0));
}
#[rstest]
fn admission_fingerprint_retains_the_existing_metadata_and_exact_ordered_media_hash_contract(
	request: ModelRequest,
) {
	let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
	let expected = json!({"request":{"instructions":"system","context":{"text":"東京"},"tools":[],"max_output_tokens":12},
        "media":[{"text":abc},{"media_type":"image/png","digest":abc},{"format":"wav","digest":abc}]});
	assert_eq!(
		request.inference_digest(),
		crate::registry::rules::digest(&expected)
	);
	assert!(
		serde_json::to_value(request)
			.unwrap()
			.get("content_parts")
			.is_none()
	);
}
#[rstest]
#[case::instructions("instructions")]
#[case::context("context")]
#[case::output_budget("output")]
#[case::media_bytes("bytes")]
#[case::media_order("order")]
#[case::media_duplicate("duplicate")]
#[case::media_type("type")]
fn every_admitted_request_or_media_change_has_a_distinct_identity(
	mut request: ModelRequest,
	#[case] change: &str,
) {
	let before = request.inference_digest();
	match change {
		"instructions" => request.instructions.push('!'),
		"context" => {
			request
				.context
				.legacy_mut()
				.expect("Legacy request context")["text"] = json!("changed")
		}
		"output" => request.max_output_tokens += 1,
		"bytes" => request.content_parts[0] = ContentPart::Text("abcd".into()),
		"order" => request.content_parts.swap(0, 1),
		"duplicate" => request.content_parts.push(request.content_parts[0].clone()),
		"type" => {
			request.content_parts[1] = ContentPart::Image {
				media_type: "image/jpeg".into(),
				bytes: b"abc".to_vec(),
			}
		}
		_ => panic!("unknown change"),
	}
	assert_ne!(request.inference_digest(), before);
}
#[rstest]
fn media_only_estimate_keeps_transmitted_array_framing_without_encoded_payload(
	mut request: ModelRequest,
) {
	request.content_parts.remove(0);
	let body = request.input_body();
	assert!(body["messages"][1]["content"].is_array());
	let payload: usize = request
		.content_parts
		.iter()
		.map(|part| match part {
			ContentPart::Image { bytes, .. } | ContentPart::Audio { bytes, .. } => {
				base64::engine::general_purpose::STANDARD
					.encode(bytes)
					.len()
			}
			ContentPart::Text(_) => 0,
		})
		.sum();
	let expected = body.to_string().len() - payload
		+ ModelRequest::media_tokens(&request.content_parts)
		+ request.max_output_tokens as usize
		+ 1024;
	assert_eq!(request.estimated_total_tokens(), expected);
}
