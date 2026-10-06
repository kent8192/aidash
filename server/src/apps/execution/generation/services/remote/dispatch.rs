//! Adapt durable dispatch use cases to the shared native bootstrap.
use super::Finalization;
use crate::{Result, federation::Federation};
use aidash_application::generation::dispatch;

pub(crate) use aidash_domain::generation::dispatch::{FinalizeInput, Input};

pub(crate) async fn finish(f: &Federation, input: &Input, result: Finalization) -> Result<()> {
	dispatch::finish(
		&crate::bootstrap::generation_dispatch_repository(&f.store),
		&crate::bootstrap::generation_dispatch_settlement(f),
		input,
		result,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn reconcile(f: &Federation) -> Result<()> {
	dispatch::reconcile(
		&crate::bootstrap::generation_dispatch_repository(&f.store),
		&crate::bootstrap::generation_dispatch_settlement(f),
	)
	.await
	.map_err(Into::into)
}
