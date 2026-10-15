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
fn an_overflow_demotes_only_a_calibrated_snapshot() {
	let mut state = EstimateState {
		snapshot: Some(EstimateSnapshot::conservative(3)),
		demoted: false,
	};
	assert!(!state.demote());
	assert!(!state.demoted);
	state.snapshot = Some(EstimateSnapshot::calibrated(3, 250_000, 20));
	assert!(state.demote());
	assert_eq!(
		state,
		EstimateState {
			snapshot: Some(EstimateSnapshot::conservative(3)),
			demoted: true,
		}
	);
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
