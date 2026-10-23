//! Management use cases.
use crate::{
	Error, Result,
	authorization::{execution, identity::Actor},
	federation::{Discovery, Federation},
	registry::Search,
};
use aidash_domain::nodes::NodeName;
use reinhardt::injectable;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct PeerManagement {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PeerManagement {
	PeerManagement { runtime }
}
impl PeerManagement {
	/// Discovery from `node` of the caller's Tenant, its default Node when none
	/// is named.
	pub(crate) async fn discover(
		&self,
		actor: Actor,
		node: Option<String>,
		query: Search,
	) -> Result<Discovery> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let node = node.as_deref().map(NodeName::parse).transpose()?;
		execution::discover(&f, &identity, node, &query).await
	}
	pub(crate) async fn identity(&self) -> Result<Value> {
		let f = self.runtime.clone();
		Ok(json!(f.config.deployment_identity()))
	}
}
