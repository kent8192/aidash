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
	}
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
