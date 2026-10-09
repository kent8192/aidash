use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn request() -> ModelRequest {
	ModelRequest {
		instructions: "system".into(),
		context: json!({"text":"東京"}),
		tools: vec![],
		max_output_tokens: 12,
		projection: Default::default(),
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
		"context" => request.context["text"] = json!("changed"),
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
