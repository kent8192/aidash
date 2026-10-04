//! Draft authority ports borrow the caller's transaction and current credential.
use super::DefinitionLookup;
use crate::Result;
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation, PolicyBundle},
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait DraftAuthority: DefinitionLookup {
	fn principal(&self) -> Principal;
	async fn lock_identity(&mut self) -> Result<()>;
	async fn share(&mut self, draft: Uuid, subject: &str) -> Result<Option<(bool, String)>>;
	async fn evaluate(&mut self, tenant: &str, evaluation: &Evaluation) -> Result<Decision>;
	async fn bundle(&mut self, tenant: &str) -> Result<PolicyBundle>;
}
