//! Fresh peer trust and bounded authority responses are independent of HTTP client types.
use crate::Result;
use aidash_domain::{federation::route::Route, semantic::Failure};
use async_trait::async_trait;
use serde_json::Value;
pub struct Reply {
	pub status: u16,
	pub semantic_reason: Option<Failure>,
	pub transaction_pending: bool,
	pub body: Result<Vec<u8>>,
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
