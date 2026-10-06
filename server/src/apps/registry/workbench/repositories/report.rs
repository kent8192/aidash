//! Factual report sources use the same native adapters as their independent HTTP and worker use cases.
use crate::{authorization::identity::Actor, federation::Federation};
use aidash_application::{Result, ports::registry::workbench::report::ReportSources};
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
pub(crate) struct Sources<'a> {
	pub(crate) runtime: &'a Federation,
	pub(crate) actor: Actor,
}
#[async_trait]
impl ReportSources for Sources<'_> {
	fn principal(&self) -> Principal {
		super::authority::principal(&self.actor)
	}
	async fn inspect(&self, reference: &EntityRef) -> Result<Inspection> {
		aidash_application::registry::workbench::inspection::usage::inspect(
			&crate::bootstrap::workbench_inspection_repository(self.runtime, self.actor.clone()),
			reference.clone(),
		)
		.await
	}
	async fn permission_context(
		&self,
		reference: &EntityRef,
		input: PermissionInput,
	) -> Result<PermissionContext> {
		aidash_application::registry::workbench::permissions::inspect(
			&crate::bootstrap::workbench_permission_repository(self.runtime, self.actor.clone()),
			reference.clone(),
			input,
		)
		.await
	}
	async fn incidents(&self, reference: &EntityRef) -> Result<Vec<Incident>> {
		aidash_application::registry::workbench::incidents::list(
			&crate::bootstrap::workbench_incident_repository(self.runtime, self.actor.clone()),
			reference.clone(),
		)
		.await
	}
}
