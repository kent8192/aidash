//! Process supervision and bounded drain, independent of HTTP and persistence.
use aidash_application::{Error, Result, lifecycle::StopToken};
use std::{future::Future, time::Duration};
use tokio::{sync::watch, task::JoinSet};

/// Owns all spawned tasks; cancellation or early return cannot detach them.
pub struct Supervisor {
	stop: watch::Sender<bool>,
	workers: JoinSet<Result<()>>,
	services: JoinSet<Result<()>>,
	drain_timeout: Duration,
}

impl Supervisor {
	pub fn new(drain_timeout: Duration) -> Self {
		let (stop, _) = watch::channel(false);
		Self {
			stop,
			workers: JoinSet::new(),
			services: JoinSet::new(),
			drain_timeout,
		}
	}

	pub fn stop_receiver(&self) -> watch::Receiver<bool> {
		self.stop.subscribe()
	}

	pub fn stop_token(&self) -> StopToken {
		StopToken::new(self.stop.subscribe())
	}

	pub fn spawn_worker(&mut self, work: impl Future<Output = Result<()>> + Send + 'static) {
		self.workers.spawn(work);
	}

	pub fn spawn_service(&mut self, work: impl Future<Output = Result<()>> + Send + 'static) {
		self.services.spawn(work);
	}

	/// Recovery services remain alive while in-flight worker effects drain.
	pub async fn shutdown(mut self) -> Result<()> {
		self.stop.send_replace(true);
		let drained = tokio::time::timeout(self.drain_timeout, async {
			while let Some(result) = self.workers.join_next().await {
				result.map_err(|error| Error::External(format!("worker failed: {error}")))??;
			}
			Ok(())
		})
		.await;
		self.workers.shutdown().await;
		self.services.shutdown().await;
		drained.map_err(|_| Error::External("worker drain deadline reached".into()))?
	}

	pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
		self.run_with_shutdown(shutdown, || {}).await
	}

	/// Close inbound admission before draining any already accepted worker steps.
	pub async fn run_with_shutdown(
		mut self,
		shutdown: impl Future<Output = ()>,
		close_admission: impl FnOnce(),
	) -> Result<()> {
		let failure = tokio::select! {
			_ = shutdown => None,
			result = self.workers.join_next(), if !self.workers.is_empty() => {
				Some(Error::External(format!("worker stopped: {result:?}")))
			},
			result = self.services.join_next(), if !self.services.is_empty() => {
				Some(Error::External(format!("background service stopped: {result:?}")))
			},
		};
		close_admission();
		let drained = self.shutdown().await;
		match failure {
			Some(error) => Err(error),
			None => drained,
		}
	}
}

impl Drop for Supervisor {
	fn drop(&mut self) {
		self.stop.send_replace(true);
		self.workers.abort_all();
		self.services.abort_all();
	}
}

pub mod generation;

pub mod semantic;

pub mod capabilities;
