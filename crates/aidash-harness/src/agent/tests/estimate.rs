//! Request Estimates pinned per Run step (ADR 0017, #179, #276).
use super::*;
use aidash_domain::provider::estimate::{
	Calibration, CalibrationSample, EstimateSnapshot, EstimateState, TokenEncoding,
};

/// Twenty samples at a quarter token per byte: enough to calibrate.
fn warm(ratio_denominator: u64) -> Vec<CalibrationSample> {
	vec![
		CalibrationSample {
			byte_basis: ratio_denominator,
			prompt_tokens: 1,
		};
		20
	]
}

fn calibrated(fixture: &mut Fixture, samples: Vec<CalibrationSample>) {
	estimating(fixture, json!("calibrated"), samples);
}

fn tokenized(fixture: &mut Fixture, samples: Vec<CalibrationSample>) {
	estimating(fixture, json!({"tokenized": "o200k_base"}), samples);
}

fn estimating(fixture: &mut Fixture, estimator: Value, samples: Vec<CalibrationSample>) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.request_estimator = Some(estimator);
	*state.calibration.lock().unwrap() = samples;
}

fn dispatches(fixture: &Fixture) -> Vec<UsageDispatch> {
	let usage = fixture.backend.0.usage.lock().unwrap();
	usage.iter().map(|(dispatch, _)| dispatch.clone()).collect()
}

#[rstest]
#[tokio::test]
async fn models_without_the_opt_in_keep_the_byte_estimate(mut fixture: Fixture) {
	// Arrange
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: nothing is queried or saved, so the Run keeps its context bytes.
	assert_eq!(count(&fixture, "calibration.samples"), 0);
	assert!(!events(&fixture).contains(&"run.estimate_pinned".to_owned()));
	assert!(fixture.run.context.estimate.is_initial());
	let [dispatch] = dispatches(&fixture).try_into().unwrap();
	let request = fixture.backend.0.requests.lock().unwrap()[0].clone();
	assert_eq!(dispatch.upstream_model, "fixture");
	assert_eq!(dispatch.estimate, request.request_estimate());
	assert_eq!(dispatch.estimate.estimator, "bytes");
	assert_eq!(dispatch.estimate.ratio_ppm, None);
	assert_eq!(
		dispatch.estimate.tokens,
		request.byte_estimate().total() as u64
	);
	assert_eq!(dispatch.estimate.output_reservation, 4096);
	assert!(dispatch.calibration_sample);
}

#[rstest]
#[tokio::test]
async fn a_calibrated_model_pins_its_snapshot_before_the_step_relies_on_it(mut fixture: Fixture) {
	// Arrange
	calibrated(&mut fixture, warm(4));
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the snapshot is durable before the attempt is dispatched.
	let calls = calls(&fixture);
	assert!(position(&calls, "calibration.samples") < position(&calls, "save"));
	assert_eq!(events(&fixture)[0], "run.estimate_pinned");
	assert!(position(&calls, "save") < position(&calls, "usage.dispatch"));
	let snapshot = EstimateSnapshot::calibrated(0, 250_000, 20);
	assert_eq!(fixture.run.context.estimate.snapshot, Some(snapshot));
	let request = fixture.backend.0.requests.lock().unwrap()[0].clone();
	assert_eq!(request.estimate, snapshot);
	let [dispatch] = dispatches(&fixture).try_into().unwrap();
	assert_eq!(dispatch.estimate.estimator, "calibrated");
	assert_eq!(dispatch.estimate.confidence, EstimateConfidence::Calibrated);
	assert_eq!(dispatch.estimate.ratio_ppm, Some(250_000));
	let bytes = request.byte_estimate();
	assert_eq!(dispatch.estimate.byte_basis, bytes.basis as u64);
	assert!(dispatch.estimate.tokens < bytes.total() as u64);
	let RunState::ToolCall(state) = &fixture.run.state else {
		panic!("inference must produce a tool-call state")
	};
	assert_eq!(state.request_tokens as u64, dispatch.estimate.tokens);
}

#[rstest]
#[case::calibrated(calibrated as fn(&mut Fixture, Vec<CalibrationSample>))]
#[case::tokenized(tokenized as fn(&mut Fixture, Vec<CalibrationSample>))]
#[tokio::test]
async fn pinning_an_estimate_keeps_resolved_research_in_the_model_request(
	mut fixture: Fixture,
	#[case] estimating: fn(&mut Fixture, Vec<CalibrationSample>),
) {
	// Arrange: this step resumes from an immutable journal reference. The
	// estimate save reloads that durable projection before fitting the request.
	estimating(&mut fixture, warm(4));
	let payload = json!({
		"contract": "aidash.research/1", "version": 1, "operation": "fetch",
		"status": "ok", "data": {"text": "the retained provider passage"},
	});
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.research_payloads
		.insert(rules::digest(&payload), payload.clone());
	fixture.run.context.push(ContextEvent::tool(
		call(
			"call-page",
			"renamed_page",
			json!({"url": "https://example.com/"}),
		),
		payload.clone(),
	));
	fixture.run.context.journal.inferred_through = fixture.run.context.journal.head;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	fixture.run = serde_json::from_value(serde_json::to_value(&fixture.run).unwrap()).unwrap();

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert: pinning is durable while the provider receives the exact payload,
	// never the journal's opaque reference envelope.
	assert_eq!(events(&fixture)[0], "run.estimate_pinned");
	let request = &fixture.backend.0.requests.lock().unwrap()[0];
	let ModelContext::Legacy(context) = &request.context else {
		panic!("the fixture uses the Legacy projection")
	};
	assert_eq!(context["history"][0]["result"], payload);
}

#[rstest]
#[tokio::test]
async fn a_resumed_step_reuses_its_snapshot_after_calibration_changed(mut fixture: Fixture) {
	// Arrange: the step pinned a snapshot, then other attempts moved the key.
	calibrated(&mut fixture, warm(2));
	let pinned = EstimateSnapshot::calibrated(0, 300_000, 20);
	fixture.run.context.estimate.snapshot = Some(pinned);
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	assert_eq!(count(&fixture, "calibration.samples"), 0);
	assert!(!events(&fixture).contains(&"run.estimate_pinned".to_owned()));
	assert_eq!(
		fixture.backend.0.requests.lock().unwrap()[0].estimate,
		pinned
	);
	let [dispatch] = dispatches(&fixture).try_into().unwrap();
	assert_eq!(dispatch.estimate.ratio_ppm, Some(300_000));
}

#[rstest]
#[tokio::test]
async fn a_cold_key_pins_the_conservative_snapshot(mut fixture: Fixture) {
	// Arrange: too few samples to calibrate.
	calibrated(&mut fixture, warm(4)[..5].to_vec());
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the conservative choice is pinned too, so a resume after the key
	// warms up still makes this step's decisions.
	assert_eq!(
		fixture.run.context.estimate.snapshot,
		Some(EstimateSnapshot::conservative(0))
	);
	assert_eq!(events(&fixture)[0], "run.estimate_pinned");
	let [dispatch] = dispatches(&fixture).try_into().unwrap();
	assert_eq!(dispatch.estimate.estimator, "bytes");
}

#[rstest]
#[case::calibrated(calibrated as fn(&mut Fixture, Vec<CalibrationSample>), "calibrated")]
#[case::tokenized(tokenized as fn(&mut Fixture, Vec<CalibrationSample>), "tokenized")]
#[tokio::test]
async fn an_overflow_after_a_scaled_estimate_demotes_the_run(
	mut fixture: Fixture,
	#[case] estimating: fn(&mut Fixture, Vec<CalibrationSample>),
	#[case] estimator: &str,
) {
	// Arrange
	estimating(&mut fixture, warm(4));
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(false));
	*state.overflows.lock().unwrap() = 1;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act: the calibrated request overflows.
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the step is demoted in the same save as the recovery state, and
	// the overflow is measured in bytes so a later one compares like with like.
	assert_eq!(events(&fixture).last().unwrap(), "run.context_overflow");
	assert_eq!(
		fixture.run.context.estimate,
		EstimateState {
			snapshot: Some(EstimateSnapshot::conservative(0)),
			demoted: true,
		}
	);
	let overflowed = fixture.backend.0.requests.lock().unwrap()[0].clone();
	assert!(!overflowed.estimate.is_conservative());
	assert_eq!(
		fixture.run.context.recovery.overflow_request_tokens,
		Some(overflowed.byte_estimate().total() as u64)
	);
	// Act: the retry and a later step use bytes without consulting calibration.
	advance_sources(&mut fixture).await.unwrap();
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	fixture.run.step += 1;
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	assert_eq!(count(&fixture, "calibration.samples"), 1);
	let estimators = dispatches(&fixture)
		.into_iter()
		.map(|dispatch| dispatch.estimate.estimator)
		.collect::<Vec<_>>();
	assert_eq!(estimators, [estimator, "bytes", "bytes"]);
	assert!(fixture.run.context.estimate.demoted);
}

#[rstest]
#[tokio::test]
async fn a_paused_overflow_keeps_the_demotion_for_the_resumed_step(mut fixture: Fixture) {
	// Arrange: without a Context Policy, an overflow pauses the Run at once.
	calibrated(&mut fixture, warm(4));
	*fixture.backend.0.overflows.lock().unwrap() = 1;
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	let paused = advance_sources(&mut fixture).await;
	// Assert: the demotion is durable before the pause, which saves no context.
	assert!(matches!(
		paused,
		Err(Error::Context(
			aidash_domain::context::recovery::Failure::OverflowRetriesExhausted
		))
	));
	let writes = fixture.backend.0.writes.lock().unwrap().clone();
	let (event, saved) = writes.last().unwrap();
	assert_eq!(event, "run.estimate_pinned");
	assert!(saved.context.estimate.demoted);
	// Act: a person resumes the same step from the saved Run.
	fixture.run = saved.clone();
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	assert_eq!(count(&fixture, "calibration.samples"), 1);
	let estimators = dispatches(&fixture)
		.into_iter()
		.map(|dispatch| dispatch.estimate.estimator)
		.collect::<Vec<_>>();
	assert_eq!(estimators, ["calibrated", "bytes"]);
}

/// A large task under large fixed instructions: the Stable Prefix snapshot
/// quota is bound by the headroom beside the fixed content.
fn ordered_with_large_task(calibration: Option<Vec<CalibrationSample>>) -> Fixture {
	let mut fixture = canonical(fixture());
	ordered(&mut fixture);
	if let Some(samples) = calibration {
		calibrated(&mut fixture, samples);
	}
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.fill_semantic_budget = true;
	state.instructions = "x".repeat(80_000);
	fixture.backend.0.task.lock().unwrap().description = "d".repeat(40_000);
	fixture
}

#[rstest]
#[tokio::test]
async fn calibration_never_moves_the_stable_prefix_or_the_semantic_budget() {
	// Arrange
	let mut bytes = ordered_with_large_task(None);
	let mut calibrated = ordered_with_large_task(Some(warm(4)));
	// Act
	for fixture in [&mut bytes, &mut calibrated] {
		assert!(matches!(
			advance_sources(fixture).await,
			Err(Error::ProviderRejected { .. })
		));
	}
	// Assert
	let conservative = bytes.backend.0.requests.lock().unwrap()[0].clone();
	let scaled = calibrated.backend.0.requests.lock().unwrap()[0].clone();
	assert!(conservative.estimate.is_conservative());
	assert!(!scaled.estimate.is_conservative());
	let (stable, _) = ordered_parts(&scaled);
	assert_eq!(stable, ordered_parts(&conservative).0);
	assert!(stable.len() < 40_000, "the snapshot quota must bind");
	assert_eq!(semantic_memory(&scaled), semantic_memory(&conservative));
	assert_eq!(scaled.cache_identity(), conservative.cache_identity());
}

#[rstest]
#[tokio::test]
async fn a_tokenized_model_counts_its_request_and_calibrates_only_its_quotas(mut fixture: Fixture) {
	// Arrange
	tokenized(&mut fixture, warm(4));
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the encoding and the quota ratio are pinned before dispatch.
	let calls = calls(&fixture);
	assert!(position(&calls, "calibration.samples") < position(&calls, "save"));
	assert_eq!(events(&fixture)[0], "run.estimate_pinned");
	let snapshot = EstimateSnapshot::tokenized(
		0,
		TokenEncoding::O200kBase,
		Some(Calibration {
			ratio_ppm: 250_000,
			samples: 20,
		}),
	);
	assert_eq!(fixture.run.context.estimate.snapshot, Some(snapshot));
	let request = fixture.backend.0.requests.lock().unwrap()[0].clone();
	assert_eq!(request.estimate, snapshot);
	let bytes = request.byte_estimate();
	let counted = bytes.tokenized.expect("the request is counted");
	let [dispatch] = dispatches(&fixture).try_into().unwrap();
	assert_eq!(dispatch.estimate.estimator, "tokenized");
	assert_eq!(dispatch.estimate.confidence, EstimateConfidence::Tokenized);
	assert_eq!(dispatch.estimate.ratio_ppm, Some(250_000));
	assert_eq!(dispatch.estimate.byte_basis, bytes.basis as u64);
	assert_eq!(
		dispatch.estimate.tokens,
		(counted + bytes.media + bytes.output + bytes.framing) as u64
	);
	assert!(dispatch.estimate.tokens < bytes.total() as u64);
	// The fixture reports fewer input tokens than estimated.
	assert!(!fixture.run.context.estimate.demoted);
}

#[rstest]
#[tokio::test]
async fn a_resumed_step_whose_encoding_is_gone_runs_on_bytes(mut fixture: Fixture) {
	// Arrange: an older worker pinned an encoding this binary no longer lists.
	tokenized(&mut fixture, warm(4));
	let retired: EstimateSnapshot = serde_json::from_value(json!({
		"step": 0,
		"estimator": {"id":"tokenized","version":1,"encoding":"cl100k_base","encoding_version":1,"margin_permille":100}
	}))
	.unwrap();
	fixture.run.context.estimate.snapshot = Some(retired);
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the step is not re-pinned from calibration; it uses bytes.
	assert_eq!(count(&fixture, "calibration.samples"), 0);
	assert_eq!(
		fixture.run.context.estimate.snapshot,
		Some(EstimateSnapshot::conservative(0))
	);
	let [dispatch] = dispatches(&fixture).try_into().unwrap();
	assert_eq!(dispatch.estimate.estimator, "bytes");
	// Act: the next step pins the encoding the model names.
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	fixture.run.step += 1;
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	assert_eq!(count(&fixture, "calibration.samples"), 1);
	assert_eq!(
		dispatches(&fixture).last().unwrap().estimate.estimator,
		"tokenized"
	);
}

#[rstest]
#[case::calibrated(calibrated as fn(&mut Fixture, Vec<CalibrationSample>), "calibrated")]
#[case::tokenized(tokenized as fn(&mut Fixture, Vec<CalibrationSample>), "tokenized")]
#[tokio::test]
async fn more_reported_input_than_estimated_demotes_the_run_from_the_next_step(
	mut fixture: Fixture,
	#[case] estimating: fn(&mut Fixture, Vec<CalibrationSample>),
	#[case] estimator: &str,
) {
	// Arrange: the provider reports far more input than any estimate.
	estimating(&mut fixture, warm(4));
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.reported_input_tokens = Some(1_000_000);
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert: the step keeps its snapshot; the demotion is saved with the step.
	let pinned = fixture.run.context.estimate.snapshot.unwrap();
	assert_eq!(pinned.id(), estimator);
	assert!(fixture.run.context.estimate.demoted);
	let writes = fixture.backend.0.writes.lock().unwrap().clone();
	assert!(writes.last().unwrap().1.context.estimate.demoted);
	// Act: the next step.
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	fixture.run.step += 1;
	advance_sources(&mut fixture).await.unwrap();
	// Assert: bytes, without consulting calibration again.
	assert_eq!(count(&fixture, "calibration.samples"), 1);
	let estimators = dispatches(&fixture)
		.into_iter()
		.map(|dispatch| dispatch.estimate.estimator)
		.collect::<Vec<_>>();
	assert_eq!(estimators, [estimator, "bytes"]);
}
