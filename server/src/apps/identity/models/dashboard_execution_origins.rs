//! Persistent dashboard_execution_origins records.

use super::AuthorizationExecution;
use crate::Result;
use crate::apps::execution::models::Run;
use crate::apps::execution::services::states::RunControl;
use crate::apps::identity::models::DashboardMapping;
use crate::authorization::identity::SubjectIdentity;
use reinhardt::db::orm::{Model, OrmExecutor, QuerySet};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "identity", table_name = "dashboard_execution_origins")]
#[derive(Serialize, Deserialize)]
pub struct DashboardExecutionOrigin {
	#[field(primary_key = true)]
	pub run_id: uuid::Uuid,
	#[field]
	pub identity_id: uuid::Uuid,
	#[field]
	pub mapping_id: uuid::Uuid,
}

impl DashboardExecutionOrigin {
	pub(crate) async fn status_waiting<E: OrmExecutor>(
		db: &mut E,
		identity: Uuid,
	) -> Result<Vec<Uuid>> {
		Ok(Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.filter_in_subquery("run_id", |runs: QuerySet<Run>| {
				runs.filter(Run::field_control().eq(RunControl::Paused))
					.filter(Run::field_error().eq(Some("identity status unavailable".to_owned())))
					.values(&["id"])
			})?
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|row| row.run_id())
			.collect())
	}

	pub(crate) async fn original_subject<E: OrmExecutor>(
		db: &mut E,
		identity: Uuid,
		run: Uuid,
	) -> Result<Option<SubjectIdentity>> {
		let origin = Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.filter(Self::field_run_id().eq(run))
			.all_with_db(db)
			.await?
			.pop();
		let Some(origin) = origin else {
			return Ok(None);
		};
		let Some(mapping) = DashboardMapping::find(db, origin.mapping_id()).await? else {
			return Ok(None);
		};
		let original = AuthorizationExecution::objects()
			.filter(AuthorizationExecution::field_run_id().eq(run))
			.filter(AuthorizationExecution::field_credential_id().eq(mapping.credential_id()))
			.exists_with_db(db)
			.await?;
		Ok(original.then(|| SubjectIdentity {
			http_session: None,
			credential_id: mapping.credential_id(),
			tenant: mapping.tenant,
			subject: mapping.subject,
		}))
	}
}

impl DashboardExecutionOrigin {}
