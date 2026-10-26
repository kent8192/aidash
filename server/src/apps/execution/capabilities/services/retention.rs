//! Native entry points adapt the unchanged retention contract to authority transactions.
use crate::{Result, authorization::access::Access, store::Store};
use aidash_application::capabilities::retention as application;
use aidash_domain::capabilities::retention::{Holder, RetainedFile, RetentionHold};
use uuid::Uuid;

/// Retains `file` of `area` for `holder` in the caller's authority transaction.
// The authority transaction type is crate-internal; this is the slice's
// internal entry point for the message and Preview callers.
#[allow(private_interfaces)]
pub async fn retain(
	store: &Store,
	access: &mut Access,
	area: Uuid,
	file: Uuid,
	holder: Holder,
) -> Result<RetentionHold> {
	application::retain(
		&mut crate::bootstrap::retention_scope(Some(store), access),
		area,
		file,
		holder,
	)
	.await
	.map_err(Into::into)
}

/// Releases every hold of `holder`, returning how many it removed.
#[allow(private_interfaces)]
pub async fn release(store: &Store, access: &mut Access, holder: &Holder) -> Result<u64> {
	application::release(
		&mut crate::bootstrap::retention_scope(Some(store), access),
		holder,
	)
	.await
	.map_err(Into::into)
}

/// The File versions the hold covering `file` keeps, when the caller may read
/// the Working Area. `None` means the version is no longer retained.
#[allow(private_interfaces)]
pub async fn retained_manifest(
	store: &Store,
	access: &mut Access,
	area: Uuid,
	file: Uuid,
) -> Result<Option<Vec<RetainedFile>>> {
	application::retained_manifest(
		&mut crate::bootstrap::retention_scope(Some(store), access),
		area,
		file,
	)
	.await
	.map_err(Into::into)
}
