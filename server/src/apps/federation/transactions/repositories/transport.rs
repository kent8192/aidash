//! Compose native peer lookup with the bounded transaction HTTP adapter.
use crate::federation::Federation;
use aidash_application::Result;
use serde::{Serialize, de::DeserializeOwned};

pub(crate) async fn remote<T: DeserializeOwned>(
	runtime: &Federation,
	node: &str,
	method: &str,
	path: &str,
	body: Option<&impl Serialize>,
) -> Result<T> {
	crate::bootstrap::peer_transport(runtime)
		.transaction(
			async { runtime.peer(node).await.map_err(Into::into) },
			method,
			path,
			body,
		)
		.await
}
