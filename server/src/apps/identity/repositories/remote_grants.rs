//! Database rows are converted explicitly into portable source execution state.
use crate::{Result, authorization::identity::SubjectIdentity};
use aidash_domain::federation::execution::{Prepared, home};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone, sqlx::FromRow)]
pub(crate) struct Grant {
	pub(crate) id: Uuid,
	pub(crate) task_id: Uuid,
	pub(crate) task_revision: i64,
	pub(crate) workspace_id: Uuid,
	pub(crate) node_id: String,
	pub(crate) tenant: String,
	pub(crate) credential_id: Uuid,
	pub(crate) root_subject: String,
	pub(crate) subject_chain: Vec<String>,
	pub(crate) inspection: Value,
	pub(crate) expires_at: DateTime<Utc>,
	pub(crate) revoked: bool,
	pub(crate) semantic: Value,
}
impl Grant {
	pub(crate) fn identity(&self) -> SubjectIdentity {
		SubjectIdentity {
			http_session: None,
			credential_id: self.credential_id,
			tenant: self.tenant.clone(),
			subject: self.root_subject.clone(),
		}
	}
	pub(crate) fn prepared(&self) -> Result<Prepared> {
		Ok(home::Grant::from(self.clone()).prepared()?)
	}
}
#[derive(Clone, sqlx::FromRow)]
pub(crate) struct HomeBinding {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub task_id: Uuid,
	pub task_revision: i64,
	pub initial_task: Value,
}

impl From<Grant> for home::Grant {
	fn from(row: Grant) -> Self {
		Self {
			id: row.id,
			task_id: row.task_id,
			task_revision: row.task_revision,
			workspace_id: row.workspace_id,
			node_id: row.node_id,
			tenant: row.tenant,
			credential_id: row.credential_id,
			root_subject: row.root_subject,
			subject_chain: row.subject_chain,
			inspection: row.inspection,
			expires_at: row.expires_at,
			revoked: row.revoked,
			semantic: row.semantic,
		}
	}
}

impl From<HomeBinding> for home::HomeBinding {
	fn from(row: HomeBinding) -> Self {
		Self {
			grant_id: row.grant_id,
			admission_id: row.admission_id,
			task_id: row.task_id,
			task_revision: row.task_revision,
			initial_task: row.initial_task,
		}
	}
}
