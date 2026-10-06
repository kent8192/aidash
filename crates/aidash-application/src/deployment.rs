//! Operator deployment observations, independent of HTTP and cluster clients.
use crate::{Error, Result, ports::DeploymentObserver};
use aidash_domain::{deployment::DeploymentStatus, identity::Principal};
use std::sync::Arc;

#[derive(Clone)]
pub struct DeploymentObservations {
	namespace: String,
	release: String,
	observer: Option<Arc<dyn DeploymentObserver>>,
}

impl DeploymentObservations {
	pub fn new(
		namespace: String,
		release: String,
		observer: Option<Arc<dyn DeploymentObserver>>,
	) -> Self {
		Self {
			namespace,
			release,
			observer,
		}
	}

	pub async fn status(&self, principal: &Principal) -> Result<DeploymentStatus> {
		if !matches!(principal, Principal::Operator) {
			return Err(Error::Forbidden);
		}
		let Some(observer) = &self.observer else {
			return Ok(DeploymentStatus::disabled());
		};
		Ok(DeploymentStatus::snapshot(
			&self.namespace,
			&self.release,
			observer.observe().await?,
		))
	}
}

#[cfg(test)]
mod tests;
