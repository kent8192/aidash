//! A dependency proof borrows the reader's live authorization transaction.
use crate::Result;
use aidash_domain::{
	Run,
	federation::{
		Peer,
		dependencies::{Checked, Input, Reference},
	},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Local visibility and peer records use the same authority snapshot, policy
/// audit, row locks, and transaction as the initiating read. Nested visibility
/// checks append edges to this scope's frontier and never make recursive RPCs.
#[async_trait]
pub trait DependencyScope: Send {
	fn node(&self) -> &str;
	fn identity(&self) -> (&str, &str);
	fn now(&self) -> Instant {
		Instant::now()
	}
	fn start_frontier(&mut self);
	fn take_frontier(&mut self) -> Vec<Reference>;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	fn catalog_resource(&self, entry: &Entry) -> Resource;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn admission(&mut self, id: Uuid, home: &str) -> Result<Option<Run>>;
	async fn bound_grant(&mut self, admission: Uuid) -> Result<Option<Uuid>>;
	async fn run_visible(&mut self, run: &Run) -> Result<bool>;
	async fn grant_visible(
		&mut self,
		execution: &str,
		grant: Uuid,
		admission: Uuid,
	) -> Result<bool>;
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	/// Retain the enabled peer's shared lock until the initiating read finishes.
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>>;
}

/// Each RPC resolves the current credential, preserves the node/protocol
/// headers, caps its full response deadline, and bounds decoding at 128 KiB.
/// Credential, transport, status, and decoding failures deny the graph proof.
#[async_trait]
pub trait DependencyTransport: Send + Sync {
	async fn check(&self, peer: &Peer, input: &Input, remaining: Duration) -> Option<Checked>;
}
