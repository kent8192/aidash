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

/// Native is a supported plan projection without relaxing the plan's stream,
/// media or cache restrictions.
#[test]
fn native_plan_models_keep_the_plan_usage_restrictions() {
	let mut model = plan_model(&["text", "image"]);
	model.projection_versions = vec![crate::projection::ProjectionVersion::Native];
	model.validate_plan_usage().unwrap();
	model
		.require_projection(crate::projection::ProjectionVersion::Native)
		.unwrap();
	for (field, value) in [
		("streaming", json!(false)),
		("cache_mode", json!("explicit")),
		("modalities", json!(["text", "audio"])),
		(
			"media_routes",
			json!([{
				"tag":"fixture", "formats":["image/png"], "source":"fixture",
				"verified_at":"2026-10-23T00:00:00Z", "expires_at":"2026-10-24T00:00:00Z"
			}]),
		),
	] {
		let mut value_model = serde_json::to_value(&model).unwrap();
		value_model[field] = value;
		let invalid: ModelConfig = serde_json::from_value(value_model).unwrap();
		assert!(invalid.validate_plan_usage().is_err(), "{field}");
	}
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
	assert!(value.get("wire_api").is_none());
}

fn openrouter_model(wire_api: Option<&str>) -> ModelConfig {
	let mut value = json!({
		"provider":"openrouter","model_id":"vendor/model",
		"endpoint":"https://openrouter.ai/api/v1",
		"provider_credential":"openrouter","context_window":8192,
		"modalities":["text"],"cost":{}
	});
	if let Some(wire_api) = wire_api {
		value["wire_api"] = json!(wire_api);
	}
	serde_json::from_value(value).unwrap()
}

#[rstest::rstest]
#[case::omitted(None, true)]
#[case::chat(Some("chat"), true)]
#[case::responses(Some("responses"), true)]
fn provider_credential_models_may_declare_the_wire_api(
	#[case] wire_api: Option<&str>,
	#[case] accepted: bool,
) {
	let model = openrouter_model(wire_api);
	assert_eq!(model.validate_wire_api().is_ok(), accepted);
	assert_eq!(
		serde_json::to_value(&model)
			.unwrap()
			.get("wire_api")
			.and_then(serde_json::Value::as_str),
		wire_api
	);
}

#[rstest::rstest]
#[case::plan_connection("chatgpt", Some("chatgpt"), Some("responses"))]
#[case::keyless("openrouter", None, Some("chat"))]
fn wire_api_requires_an_openrouter_provider_credential(
	#[case] provider: &str,
	#[case] provider_credential: Option<&str>,
	#[case] wire_api: Option<&str>,
) {
	let mut model = openrouter_model(wire_api);
	model.provider = provider.into();
	model.provider_credential = provider_credential.map(str::to_owned);
	if provider == "chatgpt" {
		model.plan_connection = Some("chatgpt".into());
	}
	assert!(model.validate_wire_api().is_err());
}

#[test]
fn omitted_wire_api_keeps_chat_transport_for_every_projection() {
	use crate::projection::ProjectionVersion;
	use crate::provider::InputFormat;
	let model = openrouter_model(None);
	for version in [
		ProjectionVersion::Legacy,
		ProjectionVersion::Ordered,
		ProjectionVersion::Native,
	] {
		assert_eq!(InputFormat::for_model(&model, version), InputFormat::Chat);
	}
}

#[test]
fn openrouter_responses_follow_the_plan_projection_mapping() {
	use crate::projection::ProjectionVersion;
	use crate::provider::InputFormat;
	let openrouter = openrouter_model(Some("responses"));
	let plan = plan_model(&["text"]);
	for (version, expected) in [
		(ProjectionVersion::Legacy, InputFormat::Chat),
		(ProjectionVersion::Ordered, InputFormat::Chat),
		(ProjectionVersion::Native, InputFormat::Responses),
	] {
		assert_eq!(InputFormat::for_model(&openrouter, version), expected);
		assert_eq!(InputFormat::for_model(&plan, version), expected);
	}
}

#[test]
fn responses_definitions_cannot_advertise_unsupported_input_or_explicit_caching() {
	let mut model = openrouter_model(Some("responses"));
	model.modalities.push("audio".into());
	assert!(
		model
			.validate_wire_api()
			.unwrap_err()
			.to_string()
			.contains("audio")
	);
	model.modalities = vec!["text".into()];
	model.model_id = "anthropic/claude-fixture".into();
	model.cache_mode = Some(CacheMode::Explicit);
	assert!(
		model
			.validate_wire_api()
			.unwrap_err()
			.to_string()
			.contains("explicit")
	);
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
