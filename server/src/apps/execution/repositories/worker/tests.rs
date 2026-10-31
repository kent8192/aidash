use super::*;
use aidash_application::route_refusal::{RouteReason, RouteRefusal};

#[test]
fn pinned_failures_fail_and_media_failures_pause_with_their_reasons() {
	let refusal = RouteRefusal {
		tag: "fixture/us".into(),
		reasons: vec![RouteReason::EndpointMissing],
	};
	let pinned = NativeError::from(Error::ProviderRouteUnavailable {
		model: "vendor/model".into(),
		refusal: refusal.clone(),
	});
	assert!(
		matches!(classify_failure(&pinned), ExecutionFailure::Other(message) if message.contains("fixture/us") && message.contains("endpoint missing"))
	);
	let media = NativeError::from(Error::MediaRouteUnavailable {
		model: "vendor/model".into(),
		candidates: vec![refusal],
	});
	assert!(
		matches!(classify_failure(&media), ExecutionFailure::MediaRoute(message) if message.contains("fixture/us") && message.contains("endpoint missing"))
	);
}
