//! Process metrics recorder and independent Prometheus listener.
use crate::{Error, Result};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use reinhardt::di::{KeyedFactoryOutput, SelfKey};
use reinhardt::server::{HttpServer, ShutdownCoordinator};
use reinhardt::{InjectionContext, ServerRouter, injectable};
use std::sync::{Arc, OnceLock};
static RECORDER: OnceLock<std::result::Result<PrometheusHandle, String>> = OnceLock::new();

/// Install once, before constructing services that emit startup metrics.
pub(crate) fn initialize_recorder() -> Result<PrometheusHandle> {
	RECORDER
		.get_or_init(|| {
			PrometheusBuilder::new()
				.install_recorder()
				.map_err(|error| format!("metrics recorder: {error}"))
		})
		.clone()
		.map_err(Error::External)
}
#[derive(Clone)]
pub struct ProcessMetrics(pub PrometheusHandle);
// Preserve a statement before the value until reinhardt-web#6441 is fixed.
#[injectable(scope = "singleton")]
pub async fn provide_metrics() -> ProcessMetrics {
	tracing::trace!(service = "ProcessMetrics", "creating injectable service");
	ProcessMetrics(
		RECORDER
			.get()
			.and_then(|result| result.as_ref().ok())
			.expect("metrics initialized at process startup")
			.clone(),
	)
}
pub async fn run(shutdown: ShutdownCoordinator) -> Result<()> {
	let handle = initialize_recorder()?;
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
