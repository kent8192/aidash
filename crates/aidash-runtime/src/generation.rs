//! Periodic generation recovery is process work, independent of the web server.
use crate::background::Passes;
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
	let mut passes = Passes::new("generation.reconciliation", Duration::from_millis(500));
	loop {
		passes
			.run(provisioning::reconcile(repository.as_ref(), &validation))
			.await;
		passes.wait().await;
	}
}
