//! Peer writes own the native connection through identity lookup and atomic registration.
use crate::Result;
use aidash_domain::{federation::Peer, registry::EntityRef};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait PeerWrite: Send {
	async fn disable(&mut self, node: &str) -> Result<Peer>;
	/// Recheck credential assignment under the registration lock and commit its audit.
	async fn register(&mut self, peer: Peer, credential: &str) -> Result<Peer>;
}
#[async_trait]
pub trait PeerConfiguration: Send + Sync {
	async fn write_scope(&self) -> Result<Box<dyn PeerWrite>>;
	async fn enabled(&self, node: &str) -> Result<Peer>;
	async fn all(&self) -> Result<Vec<Peer>>;
	async fn authorized(&self, node: &str, task: Uuid, agent: &EntityRef) -> Result<bool>;
}
#[async_trait]
pub trait PeerIdentity: Send + Sync {
	async fn identity(&self, peer: &Peer) -> Result<Value>;
}
