use super::*;
use aidash_application::route_refusal::{RouteReason, RouteRefusal};
use serde_json::Value;

/// The refusal reasons reach both the Run outcome and the Reinhardt HTTP
/// response that reports the failure.
#[test]
fn pinned_failures_fail_and_media_failures_pause_with_their_reasons() {
	let refusal = RouteRefusal {
		tag: "fixture/us".into(),
		reasons: vec![RouteReason::EndpointMissing],
	};
	let names_reasons =
		|message: &str| message.contains("fixture/us") && message.contains("endpoint missing");
	let pinned = NativeError::from(Error::ProviderRouteUnavailable {
		model: "vendor/model".into(),
		refusal: refusal.clone(),
	});
	assert!(
		matches!(classify_failure(&pinned), ExecutionFailure::Other(message) if names_reasons(&message))
	);
	let media = NativeError::from(Error::MediaRouteUnavailable {
		model: "vendor/model".into(),
		candidates: vec![refusal],
	});
	assert!(
		matches!(classify_failure(&media), ExecutionFailure::MediaRoute(message) if names_reasons(&message))
	);
	for (error, status) in [(pinned, 400), (media, 503)] {
		let response = error.http_response();
		assert_eq!(response.status.as_u16(), status);
		let body: Value = serde_json::from_slice(&response.body).unwrap();
		assert!(body["error"].as_str().is_some_and(names_reasons), "{body}");
	}
}
