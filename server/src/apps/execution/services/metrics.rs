//! Process metrics recorder and independent Prometheus listener.
use crate::{Error, Result};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use reinhardt::di::{KeyedFactoryOutput, SelfKey};
use reinhardt::server::{HttpServer, ShutdownCoordinator};
use reinhardt::{InjectionContext, ServerRouter, injectable};
use std::{
	sync::{Arc, OnceLock},
	time::Duration,
};
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
/// How often the database pools' use is sampled.
const POOL_SAMPLE: Duration = Duration::from_secs(5);

/// Records how many connections each named pool holds, how many of them are
/// idle and the most it may hold, so that waiting on the pool can be told apart
/// from waiting on a lock (#909).
fn sample_pools(pools: &[(&'static str, crate::database::native::Pool)]) {
	for (name, pool) in pools {
		let usage = pool.usage();
		let idle = usage.idle as f64;
		metrics::gauge!("aidash_db_pool_connections", "pool" => *name, "state" => "in_use")
			.set((f64::from(usage.open) - idle).max(0.0));
		metrics::gauge!("aidash_db_pool_connections", "pool" => *name, "state" => "idle").set(idle);
		metrics::gauge!("aidash_db_pool_max_connections", "pool" => *name)
			.set(f64::from(usage.max));
	}
}

pub async fn run(
	shutdown: ShutdownCoordinator,
	pools: Vec<(&'static str, crate::database::native::Pool)>,
) -> Result<()> {
	let handle = initialize_recorder()?;
	let mut stopping = shutdown.subscribe();
	let listener_handle = handle.clone();
	let listener = async move {
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
			KeyedFactoryOutput::<SelfKey<ProcessMetrics>, ProcessMetrics>::new(ProcessMetrics(
				listener_handle,
			)),
		);
		HttpServer::new(ServerRouter::new().endpoint(super::super::views::metrics::metrics))
			.with_di_context(Arc::new(context))
			.listen_on_with_shutdown(listener, shutdown)
			.await
			.map_err(|error| Error::External(format!("metrics listener: {error}")))
	};
	tokio::pin!(listener);
	let mut upkeep = tokio::time::interval(Duration::from_secs(30));
	let mut sample = tokio::time::interval(POOL_SAMPLE);
	sample.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
	loop {
		tokio::select! {
			_ = upkeep.tick() => { handle.render(); }
			_ = sample.tick() => sample_pools(&pools),
			result = &mut listener => return result,
			_ = stopping.recv() => return Ok(()),
		}
	}
}
