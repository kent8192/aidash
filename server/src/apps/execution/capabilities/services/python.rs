//! Native Python entry points preserve external DTOs while application owns heap lifecycle.
use super::{contracts::Area, operations};
pub use crate::apps::execution::capabilities::serializers::python::Python;
use crate::{Result, authorization::access::Access, store::Store};
use aidash_application::capabilities::python as application;
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn release(
	store: &Store,
	access: &mut Access,
	area: &Area,
	reason: &str,
) -> Result<()> {
	application::release(
		&mut crate::bootstrap::python_scope(Some(store), access),
		&area.into(),
		reason,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn completed(
	access: &mut Access,
	area: &Area,
	operation: &operations::Operation,
	observed: &Value,
) -> Result<()> {
	application::completed(
		&mut crate::bootstrap::python_scope(None, access),
		&area.into(),
		operation.id,
		observed,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn reap(store: &Store, cursor: &mut Uuid) -> Result<()> {
	application::reap(&crate::bootstrap::python_repository(store), cursor)
		.await
		.map_err(Into::into)
}
