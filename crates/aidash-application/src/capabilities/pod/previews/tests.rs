use super::*;
use crate::capabilities::pod::tests::{Runtime, context};
use crate::ports::capabilities::execution_pod::EgressCredential;
use std::sync::Arc;

#[test]
fn the_gateway_host_comes_from_an_address_on_the_started_port() {
	assert_eq!(host("10.0.0.7:8080", 8080).unwrap(), "10.0.0.7");
	assert_eq!(host("[fd00::7]:8080", 8080).unwrap(), "fd00::7");
	assert!(host("10.0.0.7:9090", 8080).is_err());
	assert!(host("10.0.0.7", 8080).is_err());
	assert!(host(":8080", 8080).is_err());
}

#[tokio::test]
async fn a_preview_starts_in_the_pod_with_the_run_credential() {
	let runtime = Arc::new(Runtime::default());
	let preview = Uuid::from_u128(5);
	let run = Uuid::from_u128(9);
	let host = start(
		&context(runtime.clone()),
		preview,
		run,
		"npm run dev",
		8080,
		"shared".into(),
	)
	.await
	.unwrap();
	assert_eq!(host, "10.0.0.7");
	assert_eq!(runtime.calls(), ["ensure", "preview_start"]);
	let request = runtime.previews.lock().unwrap()[0].clone();
	assert_eq!(request.operation_id, preview.to_string());
	assert_eq!(request.run_id, run.to_string());
	assert_eq!(request.code, "npm run dev");
	assert_eq!(request.port, 8080);
	assert_eq!(
		request.proxy_credential,
		Some(EgressCredential("run-credential".into()))
	);
}

#[tokio::test]
async fn stopping_a_preview_stops_its_process() {
	let runtime = Arc::new(Runtime::default());
	stop(runtime.as_ref(), Uuid::from_u128(5)).await.unwrap();
	assert_eq!(
		*runtime.stopped.lock().unwrap(),
		[Uuid::from_u128(5).to_string()]
	);
}
