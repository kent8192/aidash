//! A dependency proof borrows the reader's live authorization transaction.
use crate::Result;
use aidash_domain::{
	Run,
	federation::{
		dependencies::{Checked, Input, Reference},
		route::Route,
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
	/// The definition under `registry.read` alone, without a Catalog Approval.
	async fn readable_entry(&mut self, reference: &EntityRef) -> Result<Entry>;
	/// Retain the enabled peer's shared lock until the initiating read finishes.
	async fn route(&mut self, node: &str) -> Result<Option<Route>>;
	/// Whether a Node already failed to answer within this authority, so it is
	/// not asked again (#751).
	fn unavailable(&self, node: &str) -> bool;
	/// Remember that `node` did not answer, and that nothing is confirmed
	/// negatively while any Node is in that state. A walk calls this each time it
	/// meets such a Node, including one it did not ask again, so the call that made
	/// the walk can tell that its own refusal is that Node's silence.
	fn mark_unavailable(&mut self, node: &str);
	/// The check in progress stops at a Node already known to be silent, without
	/// asking it again, so the row it belongs to is unconfirmed (#770).
	fn skipped(&mut self, _node: &str) {}
	/// `node` answered, with a verdict or a refusal, so it is no longer remembered
	/// as silent.
	fn answered(&mut self, _node: &str) {}
}

/// What asking another Node about a reference came to (#751).
#[derive(Debug)]
pub enum Check {
	/// The peer answered with its verdict and the edges it disclosed.
	Confirmed(Checked),
	/// The peer answered `not visible`, `401`, `403` or `404`, or this
	/// Deployment refused to send.
	Refused,
	/// A timeout, `5xx`, a transport error, an unreadable answer or a pending
	/// transaction: the verdict is unknown.
	Unavailable,
	/// Nobody was asked: a same-Deployment route with no in-process runtime.
	NotAsked,
}

impl Check {
	/// What a status other than `200` means.
	pub fn of_status(status: u16) -> Self {
		if super::refuses(status) {
			Self::Refused
		} else {
			Self::Unavailable
		}
	}
}

/// Each RPC resolves the current credential, preserves the node/protocol
/// headers, caps its full response deadline, and bounds decoding at 128 KiB.
/// A refusal denies the graph proof; a failure to answer leaves it unconfirmed
/// and is never remembered as a denial.
#[async_trait]
pub trait DependencyTransport: Send + Sync {
	async fn check(&self, route: &Route, input: &Input, remaining: Duration) -> Check;
}
