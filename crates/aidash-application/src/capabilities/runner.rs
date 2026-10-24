//! Verified execution requires the current runner's matching deployment probes.
use crate::{Error, Result, ports::capabilities::runner::RunnerTransport};
use aidash_domain::capabilities::operations::runner::HealthProfile;
use serde_json::Value;
pub async fn verified_health(
	transport: &dyn RunnerTransport,
	profile: &HealthProfile,
	python: bool,
) -> Result<Value> {
	let health = transport
		.request("GET", "/v1/health", None)
		.await
		.map_err(|_| {
			Error::Conflict(
				"RUNTIME_UNAVAILABLE: start the runner and pass its deployment probes".into(),
			)
		})?;
	if !profile.matches(&health, python) {
		return Err(Error::Conflict(
			"RUNTIME_UNAVAILABLE: deployment probes do not match the execution profile".into(),
		));
	}
	Ok(health)
}
#[cfg(test)]
mod tests;
