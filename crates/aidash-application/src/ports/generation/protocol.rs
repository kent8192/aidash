//! Remote allowance admission retains authenticated authority until accounting completes.
use crate::Result;
use aidash_domain::{
	Run,
	federation::execution::Description,
	generation::{
		dispatch::Input,
		remote::{Reserved, Usage},
	},
	semantic::remote::{Operation, reuse::CitedOperation},
};
use async_trait::async_trait;
use uuid::Uuid;

/// An owned lease retains current policy and credential locks. Drop rolls back
/// unfinished work; completion preserves the native denial audit semantics.
#[async_trait]
pub trait GenerationProtocolReservation: Send {
	async fn reserve(&mut self, usage: &Usage) -> Result<Vec<Reserved>>;
	async fn finish_reservations(
		self: Box<Self>,
		result: Result<Vec<Reserved>>,
	) -> Result<Vec<Reserved>>;
}

#[async_trait]
pub trait GenerationProtocolLease: GenerationProtocolReservation {
	fn description(&self) -> &Description;
	async fn binding_admission(&mut self, grant: Uuid) -> Result<Option<Uuid>>;
	/// Recheck, under this lease, that the grant is live (`Forbidden`) and that
	/// no source it consumed was changed or revoked (`Invalidated`): the same
	/// conditions under which `grants/describe` refuses the grant.
	async fn semantic_sources_current(&mut self, grant: Uuid) -> Result<()>;
	/// This node's journal record of one semantic operation, if it has one,
	/// read through this lease's own connection.
	async fn semantic_operation(&mut self, id: Uuid) -> Result<Option<CitedOperation>>;
	/// The current corpus digest of this lease's Description Workspace,
	/// computed as for the Home's remote semantic retrievals and read through
	/// this lease's own connection.
	async fn corpus_digest(&mut self) -> Result<String>;
	async fn finish_verification(self: Box<Self>, result: Result<bool>) -> Result<bool>;
}

/// Authenticated bounded RPCs and fresh authority acquisition never invoke providers.
#[async_trait]
pub trait GenerationProtocolAuthority: Send + Sync {
	fn node_id(&self) -> &str;
	async fn leaf(
		&self,
		source: &str,
		grant: Uuid,
		admission: Uuid,
	) -> Result<Box<dyn GenerationProtocolLease>>;
	async fn grant(&self, source: &str, grant: Uuid) -> Result<Box<dyn GenerationProtocolLease>>;
	async fn worker(&self, run: &Run) -> Result<Option<Box<dyn GenerationProtocolReservation>>>;
	async fn verify_semantic(&self, description: &Description, operation: &Operation)
	-> Result<()>;
	async fn verify_peer(&self, source: &str, input: &Input) -> Result<bool>;
	async fn reserve_peer(&self, home: &str, input: &Input) -> Result<Vec<Reserved>>;
}

#[async_trait]
pub trait GenerationProtocolRepository: Send + Sync {
	/// Match both the admission ID and the Run's pinned Home node.
	async fn description(&self, run: Uuid, home: &str) -> Result<Description>;
	async fn run(&self, admission: Uuid) -> Result<Run>;
	/// Preserve the existing admission, grant, READY state and exact step predicate.
	async fn semantic_ready(&self, admission: Uuid, grant: Uuid, step: i32) -> Result<bool>;
}
