use super::*;
use serde_json::json;

fn plan_model(modalities: &[&str]) -> ModelConfig {
	serde_json::from_value(json!({
		"provider": "chatgpt",
		"model_id": "gpt-fixture",
		"endpoint": "https://api.openai.com/v1",
		"plan_connection": "chatgpt",
		"context_window": 128000,
		"max_output_tokens": 16384,
		"modalities": modalities,
		"cost": {},
	}))
	.unwrap()
}

/// A plan model has no OpenRouter routes; its declared image modality is
/// still advertised so clients can offer image uploads, and audio never is.
#[test]
fn plan_models_advertise_their_declared_image_route() {
	let routes = plan_model(&["text", "image"]).current_media_input_routes();
	assert_eq!(routes.len(), 1);
	assert!(
		routes[0]
			.iter()
			.all(|media_type| media_type.starts_with("image/"))
	);
	assert!(routes[0].contains(&"image/png".to_owned()));
	assert!(
		plan_model(&["text"])
			.current_media_input_routes()
			.is_empty()
	);
}

#[rstest::rstest]
#[case::exact("fixture/region", "approval record", -60, 3600, "openrouter", true)]
#[case::wildcard("fixture/*", "approval record", -60, 3600, "openrouter", false)]
#[case::empty_segment("fixture/", "approval record", -60, 3600, "openrouter", false)]
#[case::no_evidence("fixture/region", " ", -60, 3600, "openrouter", false)]
#[case::future("fixture/region", "approval record", 3600, 7200, "openrouter", false)]
#[case::expired("fixture/region", "approval record", -3600, -60, "openrouter", false)]
#[case::unsupported("fixture/region", "approval record", -60, 3600, "chatgpt", false)]
fn route_approval_rejects_stale_or_unsupported_pins(
	#[case] tag: &str,
	#[case] source: &str,
	#[case] verified: i64,
	#[case] expires: i64,
	#[case] provider: &str,
	#[case] accepted: bool,
) {
	let mut model = plan_model(&["text"]);
	model.provider = provider.into();
	model.plan_connection = None;
	let now = chrono::Utc::now();
	model.provider_route = Some(ProviderRouteEvidence {
		tag: tag.into(),
		source: source.into(),
		verified_at: now + chrono::Duration::seconds(verified),
		expires_at: now + chrono::Duration::seconds(expires),
	});
	assert_eq!(model.validate_provider_route().is_ok(), accepted);
}

#[rstest::rstest]
#[case::other_projection(crate::projection::ProjectionVersion::Ordered, 20, vec!["bytes"])]
#[case::not_calibrated(crate::projection::ProjectionVersion::Native, 19, vec!["bytes"])]
#[case::frozen_calibration(crate::projection::ProjectionVersion::Native, 20, vec!["bytes", "calibrated"])]
fn diagnostic_snapshots_use_only_the_frozen_projection_calibration(
	#[case] projection: crate::projection::ProjectionVersion,
	#[case] samples: u32,
	#[case] expected: Vec<&str>,
) {
	use crate::{
		projection::ProjectionVersion,
		provider::estimate::{Calibration, EstimateSnapshot},
	};
	let mut model = plan_model(&["text"]);
	model.estimate_diagnostics = Some(EstimateDiagnostics {
		token_encoding: None,
		calibrations: std::collections::BTreeMap::from([(
			ProjectionVersion::Native,
			Calibration {
				ratio_ppm: 250_000,
				samples,
			},
		)]),
	});
	model.validate_estimate_diagnostics().unwrap();
	let snapshots = model.diagnostic_snapshots(projection, 7);
	assert_eq!(
		snapshots
			.iter()
			.map(EstimateSnapshot::id)
			.collect::<Vec<_>>(),
		expected
	);
	assert!(snapshots.iter().all(|snapshot| snapshot.step == 7));
	assert_eq!(
		model.request_estimator(),
		crate::provider::estimate::RequestEstimator::Bytes
	);
}

#[rstest::rstest]
fn omitted_observability_options_do_not_add_model_fields() {
	let model = plan_model(&["text"]);
	let value = serde_json::to_value(model).unwrap();
	assert!(value.get("provider_route").is_none());
	assert!(value.get("estimate_diagnostics").is_none());
}

#[rstest::rstest]
fn pinned_media_admission_advertises_only_the_selected_route() {
	let mut model = plan_model(&["text", "image"]);
	model.provider = "openrouter".into();
	model.plan_connection = None;
	let now = chrono::Utc::now();
	let verified_at = now - chrono::Duration::minutes(1);
	let expires_at = now + chrono::Duration::hours(1);
	model.provider_route = Some(ProviderRouteEvidence {
		tag: "fixture/selected".into(),
		source: "approval".into(),
		verified_at,
		expires_at,
	});
	model.media_routes = [
		("fixture/selected", "image/png"),
		("fixture/other", "image/jpeg"),
	]
	.into_iter()
	.map(|(tag, format)| MediaRouteEvidence {
		tag: tag.into(),
		formats: vec![format.into()],
		source: "media probe".into(),
		verified_at,
		expires_at,
	})
	.collect();
	assert_eq!(
		model.current_media_input_routes(),
		vec![vec!["image/png".to_owned()]]
	);
	assert!(model.require_media_types(["image/png"]).is_ok());
	assert!(model.require_media_types(["image/jpeg"]).is_err());
}

#[rstest::rstest]
#[case::zero_ratio(0, 20, false)]
#[case::oversized_ratio(1_000_001, 20, false)]
#[case::unbounded_samples(250_000, 201, false)]
#[case::frozen_window(250_000, 200, true)]
fn diagnostic_configuration_bounds_the_frozen_inputs(
	#[case] ratio_ppm: u32,
	#[case] samples: u32,
	#[case] accepted: bool,
) {
	let mut model = plan_model(&["text"]);
	model.estimate_diagnostics = Some(EstimateDiagnostics {
		token_encoding: None,
		calibrations: std::collections::BTreeMap::from([(
			crate::projection::ProjectionVersion::Legacy,
			crate::provider::estimate::Calibration { ratio_ppm, samples },
		)]),
	});
	assert_eq!(model.validate_estimate_diagnostics().is_ok(), accepted);
}
