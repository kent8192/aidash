//! Native thread deletion adapts to the shared retention transaction.
pub use crate::apps::execution::capabilities::serializers::thread_lifecycle::DeleteThread;
pub(crate) use crate::apps::execution::repositories::thread_lifecycle::visible;
use crate::{Result, authorization::access::Access, store::Store};
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn delete(
	store: &Store,
	access: &mut Access,
	workspace: Uuid,
	thread: Uuid,
	input: DeleteThread,
) -> Result<Value> {
	aidash_application::capabilities::thread_lifecycle::delete(
		&mut crate::bootstrap::cleanup_scope(Some(store), access),
		workspace,
		thread,
		input.into(),
	)
	.await
	.map_err(Into::into)
}
