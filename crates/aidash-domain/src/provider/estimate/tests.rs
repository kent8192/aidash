use super::*;
use crate::provider::{ContentPart, ModelRequest, ToolSpec};
use rstest::{fixture, rstest};
use serde_json::json;

fn samples(ratios_ppm: impl IntoIterator<Item = u64>) -> Vec<CalibrationSample> {
	ratios_ppm
		.into_iter()
		.map(|ratio| CalibrationSample {
			byte_basis: PPM,
			prompt_tokens: ratio,
		})
		.collect()
}

#[rstest]
#[case::empty(0)]
#[case::one_short(CALIBRATION_MINIMUM - 1)]
fn too_few_samples_do_not_calibrate(#[case] count: usize) {
	assert_eq!(
		calibrate(&samples(std::iter::repeat_n(250_000, count))),
		None
	);
}

#[rstest]
fn unusable_samples_do_not_count_toward_the_minimum() {
	let mut collected = samples(std::iter::repeat_n(250_000, CALIBRATION_MINIMUM - 1));
	collected.push(CalibrationSample {
		byte_basis: 0,
		prompt_tokens: 10,
	});
	collected.push(CalibrationSample {
		byte_basis: 10,
		prompt_tokens: 0,
	});
	assert_eq!(calibrate(&collected), None);
}

#[rstest]
fn the_ratio_is_the_nearest_rank_99th_percentile() {
	// Ratios 1%..=100%: the 99th of 100 sorted ratios.
	let calibration = calibrate(&samples((1..=100).map(|percent| percent * 10_000))).unwrap();
	assert_eq!(
		calibration,
		Calibration {
			ratio_ppm: 990_000,
			samples: 100,
		}
	);
}

#[rstest]
fn only_the_newest_window_of_samples_counts() {
	// The newest CALIBRATION_WINDOW samples are low; older outliers are ignored.
	let mut collected = samples(std::iter::repeat_n(200_000, CALIBRATION_WINDOW));
	collected.extend(samples(std::iter::repeat_n(900_000, 50)));
	let calibration = calibrate(&collected).unwrap();
	assert_eq!(calibration.ratio_ppm, 200_000);
	assert_eq!(calibration.samples, CALIBRATION_WINDOW as u32);
}

#[rstest]
fn ratios_are_rounded_up_and_capped_at_one_token_per_byte() {
	let mut collected = vec![
		CalibrationSample {
			byte_basis: 3,
			prompt_tokens: 1,
		};
		CALIBRATION_MINIMUM
	];
	assert_eq!(calibrate(&collected).unwrap().ratio_ppm, 333_334);
	collected.push(CalibrationSample {
		byte_basis: 10,
		prompt_tokens: 50,
	});
	collected.push(CalibrationSample {
		byte_basis: 10,
		prompt_tokens: 50,
	});
	assert_eq!(calibrate(&collected).unwrap().ratio_ppm, PPM as u32);
}

#[fixture]
fn calibrated_samples() -> Vec<CalibrationSample> {
	samples(std::iter::repeat_n(250_000, CALIBRATION_MINIMUM))
}

#[rstest]
#[case::bytes_definition(RequestEstimator::Bytes, false, true)]
#[case::demoted_run(RequestEstimator::Calibrated, true, true)]
#[case::cold_key(RequestEstimator::Calibrated, false, false)]
fn conservative_snapshots(
	calibrated_samples: Vec<CalibrationSample>,
	#[case] estimator: RequestEstimator,
	#[case] demoted: bool,
	#[case] warm: bool,
) {
	let collected = if warm { calibrated_samples } else { vec![] };
	assert_eq!(
		EstimateSnapshot::choose(7, estimator, demoted, &collected),
		EstimateSnapshot::conservative(7)
	);
}

#[rstest]
fn an_opted_in_warm_key_pins_a_calibrated_snapshot(calibrated_samples: Vec<CalibrationSample>) {
	let snapshot =
		EstimateSnapshot::choose(7, RequestEstimator::Calibrated, false, &calibrated_samples);
	assert_eq!(snapshot, EstimateSnapshot::calibrated(7, 250_000, 20));
	assert_eq!(snapshot.id(), "calibrated");
	assert_eq!(snapshot.confidence(), EstimateConfidence::Calibrated);
	assert_eq!(snapshot.ratio_ppm(), Some(250_000));
}

#[fixture]
fn bytes() -> ByteEstimate {
	ByteEstimate {
		basis: 10_000,
		media: 4_100,
		output: 2_000,
		framing: FRAMING_RESERVE,
		tokenized: None,
	}
}

#[rstest]
fn the_byte_estimate_is_the_conservative_estimate(bytes: ByteEstimate) {
	let snapshot = EstimateSnapshot::conservative(1);
	assert_eq!(bytes.total(), 17_124);
	assert_eq!(snapshot.tokens(&bytes), bytes.total());
	assert_eq!(snapshot.tokens_for_bytes(123), 123);
	assert_eq!(snapshot.bytes_for_tokens(123), 123);
}

#[rstest]
fn calibration_scales_only_the_byte_basis(bytes: ByteEstimate) {
	// 25% plus a 10% margin: 2,750 tokens of basis; the rest is unscaled.
	let snapshot = EstimateSnapshot::calibrated(1, 250_000, 20);
	assert_eq!(
		snapshot.tokens(&bytes),
		2_750 + 4_100 + 2_000 + FRAMING_RESERVE
	);
}

#[rstest]
#[case::low(1)]
#[case::typical(250_000)]
#[case::margin_reaches_one(950_000)]
#[case::one(PPM as u32)]
fn a_calibrated_estimate_never_exceeds_bytes(bytes: ByteEstimate, #[case] ratio: u32) {
	let snapshot = EstimateSnapshot::calibrated(1, ratio, 20);
	assert!(snapshot.tokens(&bytes) <= bytes.total());
	for amount in [0, 1, 999, 10_000, 1 << 40] {
		assert!(snapshot.tokens_for_bytes(amount) <= amount);
		assert!(snapshot.tokens_for_bytes(snapshot.bytes_for_tokens(amount)) <= amount);
	}
}

#[rstest]
fn the_request_estimate_keeps_its_byte_basis_and_output_reservation(bytes: ByteEstimate) {
	let estimate = EstimateSnapshot::calibrated(1, 250_000, 20).request_estimate(&bytes);
	assert_eq!(estimate.tokens, 9_874);
	assert_eq!(estimate.estimator, "calibrated");
	assert_eq!(estimate.version, 1);
	assert_eq!(estimate.confidence, EstimateConfidence::Calibrated);
	assert_eq!(estimate.byte_basis, 10_000);
	assert_eq!(estimate.output_reservation, 2_000);
	assert_eq!(estimate.ratio_ppm, Some(250_000));
	let conservative = EstimateSnapshot::conservative(1).request_estimate(&bytes);
	assert_eq!(
		(conservative.estimator.as_str(), conservative.ratio_ppm),
		("bytes", None)
	);
	assert_eq!(conservative.confidence, EstimateConfidence::Conservative);
}

#[rstest]
fn a_pinned_snapshot_belongs_to_its_step() {
	let mut state = EstimateState::default();
	assert_eq!(state.pinned(3), None);
	assert_eq!(state.current(), EstimateSnapshot::conservative(0));
	state.snapshot = Some(EstimateSnapshot::calibrated(3, 250_000, 20));
	assert_eq!(state.pinned(3), state.snapshot);
	assert_eq!(state.pinned(4), None);
	assert_eq!(
		state.current(),
		EstimateSnapshot::calibrated(3, 250_000, 20)
	);
}

#[rstest]
fn an_overflow_demotes_a_calibrated_or_tokenized_snapshot() {
	let mut state = EstimateState {
		snapshot: Some(EstimateSnapshot::conservative(3)),
		demoted: false,
	};
	assert!(!state.demote());
	assert!(!state.demoted);
	for scaled in [
		EstimateSnapshot::calibrated(3, 250_000, 20),
		EstimateSnapshot::tokenized(3, TokenEncoding::Gemma4, None),
	] {
		state = EstimateState {
			snapshot: Some(scaled),
			demoted: false,
		};
		assert!(state.demote());
		assert_eq!(
			state,
			EstimateState {
				snapshot: Some(EstimateSnapshot::conservative(3)),
				demoted: true,
			}
		);
	}
}

#[rstest]
#[case::calibrated(EstimateSnapshot::calibrated(3, 250_000, 20))]
#[case::tokenized(EstimateSnapshot::tokenized(3, TokenEncoding::O200kBase, None))]
fn reporting_more_input_than_estimated_demotes_from_the_next_step(
	bytes: ByteEstimate,
	#[case] snapshot: EstimateSnapshot,
) {
	let estimate = snapshot.request_estimate(&bytes);
	let input = estimate.tokens - estimate.output_reservation;
	let mut state = EstimateState {
		snapshot: Some(snapshot),
		demoted: false,
	};
	assert!(!state.underestimated(&estimate, input));
	assert!(!state.demoted);
	assert!(state.underestimated(&estimate, input + 1));
	// The step keeps the snapshot its request was sent with.
	assert_eq!(
		state,
		EstimateState {
			snapshot: Some(snapshot),
			demoted: true,
		}
	);
	assert_eq!(
		EstimateSnapshot::choose(4, RequestEstimator::Calibrated, state.demoted, &[]),
		EstimateSnapshot::conservative(4)
	);
	assert!(!state.underestimated(&estimate, input + 1));
}

#[rstest]
fn a_conservative_estimate_is_never_demoted_by_an_underestimate(bytes: ByteEstimate) {
	let snapshot = EstimateSnapshot::conservative(3);
	let estimate = snapshot.request_estimate(&bytes);
	let mut state = EstimateState {
		snapshot: Some(snapshot),
		demoted: false,
	};
	assert!(!state.underestimated(&estimate, u64::MAX));
	assert!(!state.demoted);
}

#[rstest]
fn snapshots_round_trip_and_initial_state_is_omitted() {
	let state = EstimateState {
		snapshot: Some(EstimateSnapshot::calibrated(3, 250_000, 20)),
		demoted: true,
	};
	let encoded = serde_json::to_value(state).unwrap();
	assert_eq!(
		encoded,
		json!({"snapshot":{"step":3,"estimator":{"id":"calibrated","version":1,"ratio_ppm":250_000,"margin_permille":100,"samples":20}},"demoted":true})
	);
	assert_eq!(
		serde_json::from_value::<EstimateState>(encoded).unwrap(),
		state
	);
	assert!(EstimateState::default().is_initial());
	assert_eq!(
		serde_json::to_value(EstimateState::default()).unwrap(),
		json!({})
	);
}

#[rstest]
fn a_tokenized_snapshot_round_trips_and_a_retired_encoding_still_loads() {
	let snapshot = EstimateSnapshot::tokenized(
		3,
		TokenEncoding::O200kBase,
		Some(Calibration {
			ratio_ppm: 250_000,
			samples: 20,
		}),
	);
	let encoded = serde_json::to_value(snapshot).unwrap();
	assert_eq!(
		encoded,
		json!({"step":3,"estimator":{"id":"tokenized","version":1,"encoding":"o200k_base","encoding_version":1,"ratio_ppm":250_000,"margin_permille":100,"samples":20}})
	);
	assert_eq!(
		serde_json::from_value::<EstimateSnapshot>(encoded).unwrap(),
		snapshot
	);
	let retired: EstimateSnapshot = serde_json::from_value(json!({"step":3,"estimator":{"id":"tokenized","version":1,"encoding":"cl100k_base","encoding_version":1,"margin_permille":100}})).unwrap();
	assert_eq!(retired.encoding(), None);
	assert!(!retired.usable());
}

#[rstest]
fn a_tokenized_snapshot_from_another_file_version_is_unusable() {
	let mut snapshot = EstimateSnapshot::tokenized(3, TokenEncoding::Gemma4, None);
	let Estimator::Tokenized {
		encoding_version, ..
	} = &mut snapshot.estimator
	else {
		unreachable!()
	};
	*encoding_version += 1;
	assert_eq!(snapshot.encoding(), None);
	assert!(!snapshot.usable());
	assert!(EstimateSnapshot::conservative(3).usable());
	assert!(EstimateSnapshot::calibrated(3, 250_000, 20).usable());
}

#[rstest]
fn tokenized_quotas_use_the_calibrated_ratio_or_bytes() {
	let cold = EstimateSnapshot::tokenized(3, TokenEncoding::O200kBase, None);
	assert_eq!(cold.quota(), EstimateSnapshot::conservative(3));
	assert_eq!(cold.bytes_for_tokens(1_000), 1_000);
	let warm = EstimateSnapshot::tokenized(
		3,
		TokenEncoding::O200kBase,
		Some(Calibration {
			ratio_ppm: 250_000,
			samples: 20,
		}),
	);
	assert_eq!(warm.quota(), EstimateSnapshot::calibrated(3, 250_000, 20));
	assert_eq!(
		warm.bytes_for_tokens(1_000),
		EstimateSnapshot::calibrated(3, 250_000, 20).bytes_for_tokens(1_000)
	);
	assert_eq!(warm.id(), "tokenized");
	assert_eq!(warm.confidence(), EstimateConfidence::Tokenized);
	assert_eq!(warm.ratio_ppm(), Some(250_000));
}

#[rstest]
fn a_counted_basis_replaces_the_byte_basis_and_stays_capped(mut bytes: ByteEstimate) {
	let snapshot = EstimateSnapshot::tokenized(1, TokenEncoding::O200kBase, None);
	bytes.tokenized = Some(1_234);
	assert_eq!(
		snapshot.tokens(&bytes),
		1_234 + 4_100 + 2_000 + FRAMING_RESERVE
	);
	bytes.tokenized = Some(usize::MAX);
	assert_eq!(snapshot.tokens(&bytes), bytes.total());
	// Only a tokenized snapshot reads the count.
	assert_eq!(
		EstimateSnapshot::conservative(1).tokens(&bytes),
		bytes.total()
	);
}

#[rstest]
fn the_inspection_view_omits_the_sample_count() {
	let state = EstimateState {
		snapshot: Some(EstimateSnapshot::calibrated(3, 250_000, 20)),
		demoted: false,
	};
	assert_eq!(
		serde_json::to_value(state.view()).unwrap(),
		json!({"step":3,"estimator":"calibrated","version":1,"ratio_ppm":250_000,"margin_permille":100})
	);
	assert_eq!(
		serde_json::to_value(EstimateState::default().view()).unwrap(),
		json!(null)
	);
}

/// Multilingual text, escaped JSON, a large tool schema and media.
#[fixture]
fn mixed_request() -> ModelRequest {
	let properties = (0..200)
		.map(|index| {
			(
				format!("field_{index}"),
				json!({"type":"string","description":"説明 with \"quotes\" and \\ backslashes"}),
			)
		})
		.collect::<serde_json::Map<_, _>>();
	ModelRequest {
		instructions: "Follow the task.".into(),
		context: json!({
			"cjk": "東京都の天気は晴れです。한국어 텍스트. 中文文本。",
			"emoji": "👩‍👩‍👧‍👦 🇯🇵 ✨",
			"combining": "e\u{301}a\u{308}n\u{303}",
			"escaped": "{\"nested\":\"line\\nbreak\\t\\u0001\"}",
		})
		.into(),
		tools: vec![ToolSpec {
			name: "large".into(),
			description: "A large schema".into(),
			parameters: json!({"type":"object","properties":properties}),
		}],
		max_output_tokens: 4_096,
		response_format: None,
		content_parts: vec![ContentPart::Image {
			media_type: "image/png".into(),
			bytes: vec![0; 5_000],
		}],
		cache_scope: None,
		cache_breakpoints: false,
		disable_provider_transforms: false,
		input_format: Default::default(),
		estimate: Default::default(),
	}
}

#[rstest]
fn the_byte_basis_counts_utf8_bytes_of_the_escaped_body(mixed_request: ModelRequest) {
	let bytes = mixed_request.byte_estimate();
	let body = mixed_request.body(
		mixed_request
			.content_parts
			.iter()
			.map(ContentPart::estimate_frame),
		true,
	);
	assert_eq!(bytes.basis, body.to_string().len());
	assert!(bytes.basis > body.to_string().chars().count());
	assert_eq!(bytes.media, 4_096 + 5_000_usize.div_ceil(256));
	assert_eq!(bytes.output, 4_096);
	assert_eq!(mixed_request.estimated_total_tokens(), bytes.total());
}

#[rstest]
#[case::low(10_000)]
#[case::typical(300_000)]
#[case::high(PPM as u32)]
fn calibrated_requests_never_exceed_the_byte_estimate(
	mut mixed_request: ModelRequest,
	#[case] ratio: u32,
) {
	let conservative = mixed_request.estimated_total_tokens();
	mixed_request.estimate = EstimateSnapshot::calibrated(1, ratio, 20);
	assert!(mixed_request.estimated_total_tokens() <= conservative);
	assert_eq!(
		mixed_request.request_estimate().tokens,
		mixed_request.estimated_total_tokens() as u64
	);
}

#[rstest]
fn the_snapshot_never_changes_request_metadata(mut mixed_request: ModelRequest) {
	let (metadata, digest) = (
		serde_json::to_value(&mixed_request).unwrap(),
		mixed_request.inference_digest(),
	);
	mixed_request.estimate = EstimateSnapshot::calibrated(1, 250_000, 20);
	assert_eq!(serde_json::to_value(&mixed_request).unwrap(), metadata);
	assert_eq!(mixed_request.inference_digest(), digest);
}

#[rstest]
#[case::o200k_base(TokenEncoding::O200kBase)]
#[case::gemma4(TokenEncoding::Gemma4)]
fn a_tokenized_definition_pins_its_encoding_when_this_binary_has_it(
	calibrated_samples: Vec<CalibrationSample>,
	#[case] encoding: TokenEncoding,
) {
	let estimator = RequestEstimator::Tokenized(encoding);
	let cold = EstimateSnapshot::choose(7, estimator, false, &[]);
	let warm = EstimateSnapshot::choose(7, estimator, false, &calibrated_samples);
	if encoding.available() {
		assert_eq!(cold, EstimateSnapshot::tokenized(7, encoding, None));
		assert_eq!(
			warm,
			EstimateSnapshot::tokenized(
				7,
				encoding,
				Some(Calibration {
					ratio_ppm: 250_000,
					samples: 20,
				})
			)
		);
		assert_eq!(warm.encoding(), Some(encoding));
	} else {
		assert_eq!(cold, EstimateSnapshot::conservative(7));
		assert_eq!(warm, EstimateSnapshot::conservative(7));
	}
	assert_eq!(
		EstimateSnapshot::choose(7, estimator, true, &calibrated_samples),
		EstimateSnapshot::conservative(7)
	);
}

#[rstest]
fn a_definition_names_its_encoding_explicitly() {
	assert_eq!(
		serde_json::from_value::<RequestEstimator>(json!({"tokenized": "o200k_base"})).unwrap(),
		RequestEstimator::Tokenized(TokenEncoding::O200kBase)
	);
	assert_eq!(
		serde_json::to_value(RequestEstimator::Tokenized(TokenEncoding::Gemma4)).unwrap(),
		json!({"tokenized": "gemma4"})
	);
	assert!(
		serde_json::from_value::<RequestEstimator>(json!({"tokenized": "cl100k_base"})).is_err()
	);
	assert!(serde_json::from_value::<RequestEstimator>(json!("tokenized")).is_err());
}

#[cfg(feature = "token-encodings")]
#[rstest]
#[case::o200k_base(TokenEncoding::O200kBase)]
#[case::gemma4(TokenEncoding::Gemma4)]
fn tokenized_requests_count_the_body_and_never_exceed_bytes(
	mut mixed_request: ModelRequest,
	#[case] encoding: TokenEncoding,
) {
	let conservative = mixed_request.byte_estimate();
	mixed_request.estimate = EstimateSnapshot::tokenized(1, encoding, None);
	let bytes = mixed_request.byte_estimate();
	let body = mixed_request.body(
		mixed_request
			.content_parts
			.iter()
			.map(ContentPart::estimate_frame),
		true,
	);
	assert_eq!(bytes.tokenized, encoding.count_body(&body));
	assert_eq!(
		mixed_request.estimated_total_tokens(),
		bytes.tokenized.unwrap() + bytes.media + bytes.output + bytes.framing
	);
	assert!(mixed_request.estimated_total_tokens() < conservative.total());
	let estimate = mixed_request.request_estimate();
	assert_eq!(
		estimate.tokens,
		mixed_request.estimated_total_tokens() as u64
	);
	assert_eq!(estimate.estimator, "tokenized");
	assert_eq!(estimate.confidence, EstimateConfidence::Tokenized);
	// The byte basis is still recorded, so the attempt can calibrate.
	assert_eq!(estimate.byte_basis, conservative.basis as u64);
}

#[cfg(feature = "token-encodings")]
#[rstest]
fn a_tokenized_snapshot_never_changes_request_metadata(mut mixed_request: ModelRequest) {
	let (metadata, digest) = (
		serde_json::to_value(&mixed_request).unwrap(),
		mixed_request.inference_digest(),
	);
	mixed_request.estimate = EstimateSnapshot::tokenized(1, TokenEncoding::O200kBase, None);
	assert_eq!(serde_json::to_value(&mixed_request).unwrap(), metadata);
	assert_eq!(mixed_request.inference_digest(), digest);
}
