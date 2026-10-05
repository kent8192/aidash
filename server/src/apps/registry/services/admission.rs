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
