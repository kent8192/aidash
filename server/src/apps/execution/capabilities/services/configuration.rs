//! Native configuration input adapts to one shared application version transaction.
pub use crate::apps::execution::capabilities::serializers::configuration::Configure;
use crate::{Result, authorization::access::Access, store::Store};
use serde_json::Value;
pub(crate) async fn configure(
	store: &Store,
	access: &mut Access,
	id: String,
	input: Configure,
) -> Result<Value> {
	aidash_application::capabilities::configuration::configure(
		&mut crate::bootstrap::capability_configuration_scope(store, access),
		id,
		input.into(),
	)
	.await
	.map_err(Into::into)
}
