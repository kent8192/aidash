//! Management use cases.
use crate::{
	Error, Result,
	authorization::{execution, identity::Actor},
	federation::{Delegation, Federation},
};
use reinhardt::injectable;
use uuid::Uuid;

use crate::apps::federation::remote::serializers::management::DelegateInput;

#[derive(Clone)]
pub struct RemoteManagement {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> RemoteManagement {
	RemoteManagement { runtime }
}
impl RemoteManagement {
	pub(crate) async fn task_delegate(
		&self,
		actor: Actor,
		id: Uuid,
		input: DelegateInput,
	) -> Result<Delegation> {
		let Actor::Subject(identity) = actor else {
			// Tasks belong to Tenants; an Operator delegates none of its own (ADR 0043).
			return Err(Error::Forbidden);
		};
		execution::delegate(
			&self.runtime,
			&identity,
			id,
			input.node_id.as_str(),
			&input.agent,
		)
		.await
	}
}
