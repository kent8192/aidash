//! Registry admission keeps the caller-owned authority transaction.
use super::super::repositories::NativeScope;
use crate::{Result, registry::Entry};
use reinhardt::db::backends::TransactionExecutor;
pub(crate) async fn effective(
	tx: &mut dyn TransactionExecutor,
	id: &str,
	version: &str,
) -> Result<Entry> {
	Ok(aidash_application::registry::effective(&mut NativeScope(tx), id, version).await?)
}
pub(crate) async fn validate_references(
	tx: &mut dyn TransactionExecutor,
	entry: &Entry,
	node: &str,
) -> Result<()> {
	Ok(aidash_application::registry::validate_references(
		&mut NativeScope(tx),
		&crate::bootstrap::registry_validation(),
		entry,
		node,
	)
	.await?)
}
pub(crate) async fn register(
	tx: &mut dyn TransactionExecutor,
	entry: &Entry,
	node: &str,
) -> Result<bool> {
	Ok(aidash_application::registry::register_definition(
		&mut NativeScope(tx),
		&crate::bootstrap::registry_validation(),
		entry,
		node,
	)
	.await?)
}
