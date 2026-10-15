//! Registry admission keeps the caller-owned authority transaction.
use super::super::repositories::NativeScope;
use crate::{Result, registry::Entry};
use aidash_application::ports::registry::DefinitionLookup as _;
use reinhardt::db::backends::TransactionExecutor;
pub(crate) async fn definition(
	tx: &mut dyn TransactionExecutor,
	id: &str,
	version: &str,
) -> Result<Entry> {
	Ok(NativeScope(tx).definition(id, version).await?)
}
