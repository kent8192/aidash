//! Management use cases.
use crate::{
	Result,
	authorization::{execution, identity::Actor},
	federation::{Delegation, Federation},
};
use reinhardt::injectable;
use serde_json::Value;
use uuid::Uuid;

use crate::apps::federation::remote::serializers::management::DelegateInput;
use crate::apps::federation::remote::serializers::management::RemoteActionInput;

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
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			return execution::delegate(&f, &identity, id, input.node_id.as_str(), &input.agent)
				.await;
		}
		f.delegate(id, &input.node_id, &input.agent).await
	}
	pub(crate) async fn remote_action(&self, input: RemoteActionInput) -> Result<Value> {
		let f = self.runtime.clone();
		f.request(
			&input.node_id,
			reqwest::Method::POST,
			"/control",
			Some(&serde_json::to_value(input.control)?),
		)
		.await
	}
}
