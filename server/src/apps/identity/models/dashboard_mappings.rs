//! Persistent dashboard_mappings records.

use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "identity", table_name = "dashboard_mappings")]
#[derive(Serialize, Deserialize)]
pub struct DashboardMapping {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[rel(
		foreign_key,
		db_constraint = "dashboard_mappings_identity_id_fkey",
		db_index = false
	)]
	pub identity: ForeignKeyField<crate::apps::identity::models::DashboardIdentity>,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "dashboard_mappings_credential_id_fkey",
		db_index = false
	)]
	pub credential: ForeignKeyField<crate::apps::identity::models::AuthorizationCredential>,
	#[field(default = true)]
	pub enabled: bool,
	#[field(default = 1)]
	pub revision: i64,
}

impl DashboardMapping {
	pub(crate) async fn enabled_for_identity<E: OrmExecutor>(
		db: &mut E,
		identity: Uuid,
	) -> crate::Result<Vec<Self>> {
		Ok(Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.filter(Self::field_enabled().eq(true))
			.order_by(&["tenant", "subject", "id"])
			.all_with_db(db)
			.await?)
	}
}

impl DashboardMapping {}
