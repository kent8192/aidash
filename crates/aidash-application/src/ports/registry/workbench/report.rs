//! Report sources retain their own current inspection, execution-context and incident authorization.
use crate::Result;
use aidash_domain::{
	identity::Principal,
	registry::{
		EntityRef,
		workbench::{
			incident::Incident,
			inspection::Inspection,
			permissions::{PermissionContext, PermissionInput},
		},
	},
};
use async_trait::async_trait;
#[async_trait]
pub trait ReportSources: Send + Sync {
	fn principal(&self) -> Principal;
	async fn inspect(&self, reference: &EntityRef) -> Result<Inspection>;
	async fn permission_context(
		&self,
		reference: &EntityRef,
		input: PermissionInput,
	) -> Result<PermissionContext>;
	async fn incidents(&self, reference: &EntityRef) -> Result<Vec<Incident>>;
}
