//! Fresh peer trust and bounded authority responses are independent of HTTP client types.
use crate::Result;
use aidash_domain::{federation::route::Route, semantic::Failure};
use async_trait::async_trait;
use serde_json::Value;
pub struct Reply {
	pub status: u16,
	pub semantic_reason: Option<Failure>,
	pub execution_refusal: Option<aidash_domain::federation::execution::Refusal>,
	pub transaction_pending: bool,
	/// The peer answered `503 node_unreachable`: another Node could not be
	/// reached, with whether the call had already taken effect (#770).
	pub unreachable: Option<Unreachable>,
	/// A refused Authority Change, with the seconds until it can be sent again.
	pub policy_busy: Option<u64>,
	pub body: Result<Vec<u8>>,
}
/// What a peer's `node_unreachable` answer says: only the Node and whether the
/// call had taken effect, never the peer's text.
pub struct Unreachable {
	pub node: String,
	pub after_effect: bool,
}
impl Unreachable {
	/// Read the fields of a peer's error envelope, if it is a `node_unreachable`.
	pub fn of(envelope: &Value) -> Option<Self> {
		(envelope["code"] == "node_unreachable")
			.then(|| Self {
				node: envelope["node"].as_str().unwrap_or_default().to_owned(),
				after_effect: envelope["after_effect"].as_bool().unwrap_or(true),
			})
			.filter(|found| !found.node.is_empty())
	}
}
#[async_trait]
pub trait Peers: Send + Sync {
	async fn route(
		&self,
		node: &str,
		request: &crate::federation::inbound::RequestDigest<'_>,
	) -> Result<Route>;
}
#[async_trait]
pub trait Transport: Send + Sync {
	async fn request(&self, route: &Route, path: &str, body: &Value) -> Result<Reply>;
}
