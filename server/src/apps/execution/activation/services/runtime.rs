//! Native compatibility entry points assemble the shared process driver.
use super::Settings;
use crate::{Result, federation::Federation, harness::Harness};
use std::sync::Arc;
use tokio::sync::watch;
pub struct Runtime {
	driver: Arc<aidash_runtime::activation::Runtime>,
}
impl Runtime {
	pub fn new(mut federation: Federation, settings: Settings, worker: bool) -> Arc<Self> {
		if let Ok(url) = std::env::var("AIDASH_ACTIVATION_NATS_URL") {
			federation.config.nats_url = url;
		}
		Arc::new(Self {
			driver: crate::bootstrap::activation_driver(federation, settings, worker),
		})
	}
	pub async fn run(self: Arc<Self>, stopping: watch::Receiver<bool>) -> Result<()> {
		self.driver.clone().run(stopping).await.map_err(Into::into)
	}
	pub async fn worker(
		self: Arc<Self>,
		harness: Harness,
		stopping: watch::Receiver<bool>,
	) -> Result<()> {
		self.driver
			.clone()
			.worker(
				Arc::new(crate::bootstrap::activation_repository(harness)),
				stopping,
			)
			.await
			.map_err(Into::into)
	}
}
