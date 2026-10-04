//! Process metrics recorder and independent Prometheus listener.
use crate::{Error, Result};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use reinhardt::di::{KeyedFactoryOutput, SelfKey};
use reinhardt::server::{HttpServer, ShutdownCoordinator};
use reinhardt::{InjectionContext, ServerRouter, injectable};
use std::sync::{Arc, OnceLock};
static RECORDER: OnceLock<PrometheusHandle> = OnceLock::new();
#[derive(Clone)]
pub struct ProcessMetrics(pub PrometheusHandle);
// Preserve a statement before the value until reinhardt-web#6441 is fixed.
#[injectable(scope = "singleton")]
pub async fn provide_metrics() -> ProcessMetrics {
	tracing::trace!(service = "ProcessMetrics", "creating injectable service");
	ProcessMetrics(
		RECORDER
			.get()
			.expect("metrics initialized at process startup")
			.clone(),
	)
}
pub async fn run(shutdown: ShutdownCoordinator) -> Result<()> {
	let handle = if let Some(handle) = RECORDER.get() {
		handle.clone()
	} else {
		let handle = PrometheusBuilder::new()
			.install_recorder()
			.map_err(|error| Error::External(format!("metrics recorder: {error}")))?;
		let _ = RECORDER.set(handle.clone());
		handle
	};
	let Ok(address) = std::env::var("AIDASH_METRICS_LISTEN") else {
		std::future::pending::<()>().await;
		return Ok(());
	};
	let address: std::net::SocketAddr = address
		.parse()
		.map_err(|_| Error::Invalid("invalid AIDASH_METRICS_LISTEN".into()))?;
	let listener = tokio::net::TcpListener::bind(address).await?;
	let context = InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	context.set_singleton(
		KeyedFactoryOutput::<SelfKey<ProcessMetrics>, ProcessMetrics>::new(ProcessMetrics(handle)),
	);
	HttpServer::new(ServerRouter::new().endpoint(super::super::views::metrics::metrics))
		.with_di_context(Arc::new(context))
		.listen_on_with_shutdown(listener, shutdown)
		.await
		.map_err(|error| Error::External(format!("metrics listener: {error}")))
}
