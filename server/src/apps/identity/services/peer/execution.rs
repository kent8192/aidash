//! Native peer preflight adapts the caller's existing transaction to application policy.
pub(crate) use crate::apps::identity::serializers::peer_execution::{InspectInput, Inspection};
use crate::{Result, authorization::access::Access, federation::Federation};
use http::HeaderMap;
use reinhardt::injectable;
pub(crate) async fn inspect_in(
	f: &Federation,
	access: &mut Access,
	node: &str,
	input: &InspectInput,
) -> Result<Inspection> {
	aidash_application::authorization::peer::execution::inspect(
		&mut crate::bootstrap::peer_inspection_scope(f, access),
		node,
		input,
	)
	.await
	.map_err(Into::into)
}
#[derive(Clone)]
pub struct PeerExecution {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide_execution(#[inject] runtime: Federation) -> PeerExecution {
	PeerExecution { runtime }
}
impl PeerExecution {
	pub(crate) async fn inspect(
		&self,
		headers: HeaderMap,
		input: InspectInput,
	) -> Result<Inspection> {
		let f = self.runtime.clone();
		let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
		let mut access = super::access(&f, node, &input.tenant, &input.subject).await?;
		let result = inspect_in(&f, &mut access, node, &input).await;
		access.finish(result).await
	}
}
