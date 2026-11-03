use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn request() -> ModelRequest {
	ModelRequest {
		provider_tools: vec![],
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
		input_format: Default::default(),
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
#[case::input_format("input_format")]
fn every_admitted_request_or_media_change_has_a_distinct_identity(
	mut request: ModelRequest,
	#[case] change: &str,
) {
	let before = request.inference_digest();
	match change {
		"input_format" => {
			let mut metadata = serde_json::to_value(&request).unwrap();
			metadata["input_format"] = json!("responses");
			let parts = std::mem::take(&mut request.content_parts);
			request = serde_json::from_value(metadata).unwrap();
			request.content_parts = parts;
		}
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

#[rstest]
fn responses_fitting_counts_the_sent_input_shape() {
	let request: ModelRequest = serde_json::from_value(json!({
		"instructions":"Follow the task.",
		"context":{"task":"東京 \"quoted\""},
		"tools":[],
		"max_output_tokens":12,
		"input_format":"responses",
	}))
	.unwrap();
	// The independently specified Responses payload, before transport flags.
	let sent = json!({
		"instructions":"Follow the task.",
		"input":[{"role":"user","content":[{
			"type":"input_text","text":"{\"task\":\"東京 \\\"quoted\\\"\"}"
		}]}],
	});
	let required = sent.to_string().len() + 12 + estimate::FRAMING_RESERVE;
	assert_eq!(request.estimated_total_tokens(), required);
	assert!(request.ensure_fits(required).is_ok());
	assert!(request.ensure_fits(required - 1).is_err());
}

#[rstest]
#[case::audio(true, "unsupported Responses input")]
#[case::format(false, "unsupported response format")]
fn unsupported_responses_input_never_passes_admission(#[case] audio: bool, #[case] reason: &str) {
	let mut request = ModelRequest {
		input_format: InputFormat::Responses,
		..Default::default()
	};
	if audio {
		request.content_parts.push(ContentPart::Audio {
			format: "wav".into(),
			bytes: b"RIFF0000WAVE".to_vec(),
		});
	} else {
		request.response_format = Some(json!({"type":"unsupported"}));
	}
	if audio {
		assert!(
			ModelRequest::content_parts_reservation(
				&request.content_parts,
				crate::projection::ProjectionVersion::Native,
				InputFormat::Responses
			)
			.unwrap_err()
			.to_string()
			.contains(reason)
		);
	}
	assert_eq!(request.estimated_total_tokens(), usize::MAX);
	assert!(
		request
			.ensure_fits(usize::MAX)
			.unwrap_err()
			.to_string()
			.contains(reason)
	);
	assert!(request.validate().unwrap_err().to_string().contains(reason));
}

#[rstest]
fn native_responses_media_reserves_escaped_frames_but_not_base64_payloads() {
	let mut request = ModelRequest {
		input_format: InputFormat::Responses,
		context: ModelContext::Native(crate::projection::native::native_context(
			&crate::context::Context::default(),
			&json!({"run_messages":["東京"]}),
		)),
		..Default::default()
	};
	let before = request.estimated_total_tokens();
	let bytes = b"\x89PNG\r\n\x1a\nimage".to_vec();
	let encoded = base64::engine::general_purpose::STANDARD
		.encode(&bytes)
		.len();
	request.content_parts = vec![
		ContentPart::Text("image \"東京\"".into()),
		ContentPart::Image {
			media_type: "image/png".into(),
			bytes,
		},
	];
	let sent = responses::encoded_input_body(&request).unwrap();
	let required = sent.to_string().len() - encoded + 4097 + estimate::FRAMING_RESERVE;
	assert_eq!(request.estimated_total_tokens(), required);
	assert_eq!(
		ModelRequest::content_parts_reservation(
			&request.content_parts,
			crate::projection::ProjectionVersion::Native,
			InputFormat::Responses
		)
		.unwrap(),
		required - before
	);
}
