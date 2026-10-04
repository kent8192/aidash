//! Native patch requests adapt to the shared application transaction.
use super::contracts::{Area, Patch};
use crate::{Result, authorization::access::Access, domain::Run, store::Store};
use serde_json::Value;
pub(crate) async fn apply(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Patch,
) -> Result<Value> {
	let mut state = (&*area).into();
	let result = aidash_application::capabilities::patch::apply(
		&mut crate::bootstrap::file_scope(Some(store), access, Some(run)),
		&run.metadata(),
		&mut state,
		input.into(),
	)
	.await;
	*area = state.into();
	result.map_err(Into::into)
}
