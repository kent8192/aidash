//! Native offline installation adapters retain unchanged request and result contracts.
use super::{
	contracts::{Area, FileEntry},
	operations,
};
pub use crate::apps::execution::capabilities::serializers::packages::{Install, Wheel};
use crate::{Result, authorization::access::Access, domain::Run, store::Store};
use aidash_application::capabilities::packages as application;
use serde_json::{Value, json};
pub(crate) async fn authorize(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	wheels: &[Wheel],
) -> Result<Vec<FileEntry>> {
	application::authorize(
		&mut crate::bootstrap::package_scope(store, access, Some(run)),
		&run.metadata(),
		&area.into(),
		&wheels.iter().cloned().map(Into::into).collect::<Vec<_>>(),
	)
	.await
	.map(|v| v.into_iter().map(Into::into).collect())
	.map_err(Into::into)
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Install,
) -> Result<Value> {
	let mut state = (&*area).into();
	let result = application::prepare(
		&mut crate::bootstrap::package_scope(store, access, Some(run)),
		&run.metadata(),
		&mut state,
		input.into(),
	)
	.await;
	*area = state.into();
	result.map_err(Into::into)
}
pub(crate) fn inputs(operation: &operations::Operation) -> Result<Vec<FileEntry>> {
	Ok(serde_json::from_value(
		operation
			.input
			.get("package_files")
			.cloned()
			.unwrap_or(json!([])),
	)?)
}
pub(crate) async fn environment(store: &Store, access: &mut Access, area: &Area) -> Result<Value> {
	application::environment(
		&mut crate::bootstrap::package_scope(store, access, None),
		&area.into(),
	)
	.await
	.map_err(Into::into)
}
