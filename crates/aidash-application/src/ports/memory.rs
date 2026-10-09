//! Authority-scoped memory operations. Database adapters own atomicity and live policy locks.
use crate::Result;
use aidash_domain::{memory::*, registry::EntityRef};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Native adapters retain this ledger outside recoverable database snapshots.
/// Only memory paths consult its serving gate; primary execution remains usable.
pub trait MemoryRecovery: Send + Sync {
	fn require_serving(&self) -> Result<()>;
	fn epoch(&self) -> Result<Uuid>;
	/// Every canonical read also checks the independently retained body fence.
	/// A database rewind or uncertain commit cannot be served with an open epoch.
	fn require_current(&self, unit: &Unit) -> Result<()>;
	fn observe_many(&self, units: &[Unit]) -> Result<()>;
}

/// A snapshot contains only currently readable units and fully resolved source revisions.
#[derive(Debug, Clone)]
pub struct Snapshot {
	pub units: Vec<Unit>,
	/// Disposable edges are separate from canonical unit content/digests.
	pub graph: Vec<graph::Edge>,
	pub authority_revision: String,
}

#[async_trait]
pub trait MemoryScope: Send {
	/// Verify the Home, tenant, participant binding, provider version and requested action.
	async fn authorize(&mut self, bank: &Bank, provider: &EntityRef, action: &str) -> Result<()>;
	async fn snapshot(&mut self, bank: &Bank, limit: usize) -> Result<Snapshot>;
	/// Recall-only filtering; maintenance and evidence keep the complete visible snapshot.
	async fn recall_snapshot(&mut self, bank: &Bank, limit: usize) -> Result<Snapshot> {
		self.snapshot(bank, limit).await
	}
	/// Independently bounded and explicitly authorized dormant-only snapshot.
	async fn dormant_snapshot(&mut self, _bank: &Bank, _limit: usize) -> Result<Snapshot> {
		Err(crate::Error::Forbidden)
	}
	/// Capture active and Dormant membership in one retention snapshot. Each
	/// partition is independently bounded by limit; the graph covers the union.
	/// Adapters without explicit Dormant authority must fail closed.
	async fn recall_including_dormant_snapshot(
		&mut self,
		_bank: &Bank,
		_limit: usize,
	) -> Result<Snapshot> {
		Err(crate::Error::Forbidden)
	}
	async fn retention_scores(
		&mut self,
		_bank: &Bank,
		_units: &[Unit],
	) -> Result<std::collections::BTreeMap<Uuid, f64>> {
		Ok(std::collections::BTreeMap::new())
	}
	/// Successful support selection reactivates recall state in the owning transaction.
	async fn reactivate_support(&mut self, _bank: &Bank, _units: &[Unit]) -> Result<()> {
		Ok(())
	}
	/// Recheck current source revisions, disclosure and deletion fences, including remote Home receipts.
	async fn current(&mut self, bank: &Bank, evidence: &[Evidence]) -> Result<()>;
	/// Must atomically reauthorize, compare revisions, check evidence, fence dependents,
	/// enforce bank storage caps and persist exact-request idempotency + unit history.
	/// An identical replay returns the original result. A changed request with the same ID fails.
	async fn mutate(&mut self, mutation: &Mutation, bounds: &Bounds) -> Result<Vec<Unit>>;
	/// Persist candidates without admitting them. Only complete, opted-in eligible Runs pass here.
	async fn propose(
		&mut self,
		bank: &Bank,
		run: &Evidence,
		candidates: &[Content],
		bounds: &Bounds,
	) -> Result<Vec<Candidate>>;
	/// Review and mutation share one authority transaction. No model can call this admission port.
	async fn review(
		&mut self,
		candidate: Uuid,
		expected_revision: i64,
		mutation: Option<&Mutation>,
		bounds: &Bounds,
	) -> Result<Option<Unit>>;
	/// Explicit disclosure-checked copy, preserving exact source lineage and invalidation dependencies.
	async fn publish(
		&mut self,
		source: &Evidence,
		mutation: &Mutation,
		bounds: &Bounds,
	) -> Result<Vec<Unit>>;
	/// Return IDs within the authorized snapshot, never an unfiltered search result.
	async fn semantic(
		&mut self,
		bank: &Bank,
		model: &EntityRef,
		text: &str,
		allowed: &[Uuid],
		limit: usize,
		allowance: Allowance,
	) -> Result<Produced<Vec<Uuid>>>;
	async fn keyword(
		&mut self,
		bank: &Bank,
		text: &str,
		allowed: &[Uuid],
		limit: usize,
	) -> Result<Vec<Uuid>>;
	/// Revalidate authorization and each selected Unit's exact revision and content provenance.
	/// Evidence contains selected Unit roots only; validate each root's content with its
	/// admitted graph bound, then stage those roots until the complete delivery succeeds.
	/// Failed recall/reflection or combined context must not persist read dependencies.
	/// A policy change requires a retry.
	async fn deliver(
		&mut self,
		bank: &Bank,
		authority_revision: &str,
		evidence: &[Evidence],
	) -> Result<()>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Allowance {
	pub calls: usize,
	pub tokens: usize,
	pub cost_micros: u64,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Usage {
	pub tokens: usize,
	pub cost_micros: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Produced<T> {
	pub output: T,
	pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "step", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReflectStep {
	Recall { query: RecallQuery },
	Read { id: Uuid, revision: i64 },
	Answer { reflection: Reflection },
}

/// Role bindings must resolve the exact Registry version. There is no implicit provider fallback.
/// Before external calls, reserve the supplied bounded allowance through the origin budget ledger.
#[async_trait]
pub trait MemoryModels: Send + Sync {
	/// Resolve the exact reranker binding before reserving a provider call.
	/// Local rerankers receive zero allowance and must report zero usage.
	fn reranker_uses_model(&self, model: &EntityRef) -> Result<bool>;
	/// Select semantically equivalent/related support without dropping mandatory
	/// facts or conflicts. Return only supplied exact Unit evidence identities.
	async fn consolidate(
		&self,
		model: &EntityRef,
		mandatory: &[Unit],
		candidates: &[Unit],
		bounds: &Bounds,
		allowance: Allowance,
	) -> Result<Produced<Vec<Evidence>>>;
	async fn extract(
		&self,
		model: &EntityRef,
		text: &str,
		evidence: &[Evidence],
		mode: extraction::Mode,
		bounds: &Bounds,
		allowance: Allowance,
	) -> Result<Produced<extraction::Extraction>>;
	async fn derive(
		&self,
		model: &EntityRef,
		kind: Kind,
		mental_model: Option<&MentalModel>,
		units: &[Unit],
		bounds: &Bounds,
		allowance: Allowance,
	) -> Result<Produced<Content>>;
	async fn rerank(
		&self,
		model: &EntityRef,
		query: &str,
		units: &[Unit],
		allowance: Allowance,
	) -> Result<Produced<Vec<(Uuid, f64)>>>;
	async fn reflect(
		&self,
		model: &EntityRef,
		query: &str,
		context: &[Unit],
		bounds: &Bounds,
		allowance: Allowance,
	) -> Result<Produced<ReflectStep>>;
	/// Count the entire serialized contribution, including labels, revisions and provenance.
	async fn tokens(&self, tokenizer: &EntityRef, envelope: &str) -> Result<usize>;
}
