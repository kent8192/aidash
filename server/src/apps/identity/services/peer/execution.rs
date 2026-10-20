//! Native peer preflight adapts the caller's existing transaction to application policy.
pub(crate) use crate::apps::identity::serializers::peer_execution::{InspectInput, Inspection};
use crate::{Result, authorization::access::Access, federation::Federation};
use aidash_application::federation::inbound::Admitted;
use reinhardt::injectable;
pub(crate) async fn inspect_in(
	f: &Federation,
	access: &mut Access,
	receiver: &aidash_domain::nodes::NodeName,
	node: &str,
	input: &InspectInput,
) -> Result<Inspection> {
	aidash_application::authorization::peer::execution::inspect(
		&mut crate::bootstrap::peer_inspection_scope(f, access, receiver.clone()),
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
		admitted: Admitted,
		input: InspectInput,
	) -> Result<Inspection> {
		let f = self.runtime.clone();
		let node = &admitted.sender.to_string();
		let mut access = super::access(&f, &admitted).await?;
		let result = inspect_in(&f, &mut access, &admitted.receiver.node, node, &input).await;
		access.finish(result).await
	}
}
