//! Bounded, source-attributed factual history for an exact agent version.
use super::*;
use crate::registry::EntityRef;
use reinhardt::injectable;

pub(crate) use crate::apps::registry::workbench::serializers::audit::AuditQuery;
pub use crate::apps::registry::workbench::serializers::audit::{AuditItem, AuditPage};

#[derive(Clone)]
pub struct AuditHistory {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_audit(#[inject] runtime: Federation) -> AuditHistory {
	AuditHistory { runtime }
}

impl AuditHistory {
	pub(crate) async fn audit(
		&self,
		actor: Actor,
		(id, version): (String, String),
		query: AuditQuery,
	) -> Result<AuditPage> {
		aidash_application::registry::workbench::audit::read(
			&crate::bootstrap::workbench_audit_repository(&self.runtime, actor),
			EntityRef { id, version },
			query,
		)
		.await
		.map_err(Into::into)
	}
}
