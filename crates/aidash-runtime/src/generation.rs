//! Periodic generation recovery is process work, independent of the web server.
use aidash_application::{
	Result, generation::provisioning, ports::generation::provisioning::GenerationProvisioning,
	registry::DefinitionValidation,
};
use std::{sync::Arc, time::Duration};

/// Recovery remains alive while workers drain; the owning supervisor cancels it.
pub async fn run(
	repository: Arc<dyn GenerationProvisioning>,
	validation: DefinitionValidation,
) -> Result<()> {
	loop {
		if let Err(error) = provisioning::reconcile(repository.as_ref(), &validation).await {
			tracing::warn!(%error, "generation reconciliation failed");
		}
		tokio::time::sleep(Duration::from_millis(500)).await;
	}
}
