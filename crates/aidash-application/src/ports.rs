//! Ports describe atomic business operations, not database query syntax.
use crate::{Result, authorization::Snapshot};
use aidash_domain::policy::{Decision, Evaluation, PolicyBundle};
use async_trait::async_trait;

/// A transaction keeps policy locks until commit or rollback on drop.
/// Implementations must use the same transaction for the policy, its history,
/// and the decision audit. Cancellation must roll back uncommitted writes.
#[async_trait]
pub trait AuthorizationScope: Send {
	async fn load(&mut self, tenant: &str) -> Result<Snapshot>;
	async fn record_decision(
		&mut self,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()>;
}

/// An owned transaction can replace policy and commit; borrowed scopes cannot.
#[async_trait]
pub trait AuthorizationTransaction: AuthorizationScope {
	async fn replace(
		&mut self,
		tenant: &str,
		expected_revision: i64,
		bundle: &PolicyBundle,
		actor: &str,
	) -> Result<i64>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait AuthorizationStore: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn AuthorizationTransaction>>;
}

#[async_trait]
pub trait ModelCatalog: Send + Sync {
	async fn models(&self) -> Result<Vec<aidash_domain::catalog::CatalogModel>>;
}

/// One inference includes validation and the complete response body deadline.
#[async_trait]
pub trait ModelProvider: Send + Sync {
	async fn infer(
		&self,
		request: aidash_domain::provider::ModelRequest,
	) -> Result<aidash_domain::provider::ModelResponse>;
}

/// Resolve a named credential at use time so rotation does not require a restart.
pub trait Credentials: Send + Sync {
	fn resolve(&self, reference: &str) -> Result<String>;
}

/// Return a complete observation; partial pages or partial resource lists fail.
#[async_trait]
pub trait DeploymentObserver: Send + Sync {
	async fn observe(&self) -> Result<aidash_domain::deployment::DeploymentInventory>;
}

/// Probability questions contain a bounded classification view, not an execution command.
pub type CompactionQuestions = serde_json::Map<String, serde_json::Value>;
#[async_trait]
pub trait CompactionClassifier: Send + Sync {
	async fn ask(
		&self,
		state: &serde_json::Value,
		questions: &CompactionQuestions,
	) -> Result<serde_json::Value>;
}

/// Embedding output must match the approved model and vector dimensions.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
	async fn embed(
		&self,
		config: &aidash_domain::semantic::EmbeddingConfig,
		text: &str,
	) -> Result<aidash_domain::semantic::Embedding>;
}

/// Physical vectors provide candidates, never authorization or source truth.
#[async_trait]
pub trait VectorIndex: Send + Sync {
	async fn ensure_collection(
		&self,
		config: &aidash_domain::semantic::VectorConfig,
		collection: &str,
		dimensions: usize,
	) -> Result<()>;
	async fn upsert(
		&self,
		config: &aidash_domain::semantic::VectorConfig,
		collection: &str,
		point: uuid::Uuid,
		vector: &[f32],
		payload: serde_json::Value,
	) -> Result<()>;
	async fn delete_point(
		&self,
		config: &aidash_domain::semantic::VectorConfig,
		collection: &str,
		point: uuid::Uuid,
	) -> Result<()>;
	async fn delete_collection(
		&self,
		config: &aidash_domain::semantic::VectorConfig,
		collection: &str,
	) -> Result<()>;
	async fn query(
		&self,
		config: &aidash_domain::semantic::VectorConfig,
		collection: &str,
		vector: &[f32],
		filter: aidash_domain::semantic::VectorFilter<'_>,
		limit: usize,
	) -> Result<Vec<aidash_domain::semantic::Point>>;
	async fn present(
		&self,
		config: &aidash_domain::semantic::VectorConfig,
		collection: &str,
		ids: &[uuid::Uuid],
	) -> Result<bool>;
}

/// Recovery updates must atomically retain the original worker lease fence,
/// revision check, typed state, audit event, and terminal-delivery outbox.
#[async_trait]
pub trait ExecutionRecoveryStore: Send + Sync {
	async fn leased_run(&self, token: uuid::Uuid) -> Result<Option<aidash_domain::Run>>;
	async fn save(&self, run: &aidash_domain::Run, token: uuid::Uuid, event: &str) -> Result<()>;
	async fn pause(
		&self,
		run: &aidash_domain::Run,
		token: uuid::Uuid,
		reason: &str,
		event: &str,
	) -> Result<()>;
	async fn pause_semantic(
		&self,
		run: &aidash_domain::Run,
		token: uuid::Uuid,
		reason: aidash_domain::semantic::Failure,
	) -> Result<()>;
}

pub mod execution;

pub mod events;

pub mod federation;
pub mod tools;

pub mod registry;

pub mod workspaces;

pub mod channels;

pub mod marketplace;

pub mod catalog;

pub mod generation;

pub mod semantic;

pub mod authorization;

pub mod graph;

pub mod capabilities;

pub mod transactions;

pub mod activation;
pub mod bindings;
