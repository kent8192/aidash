//! Persistent authorization_credentials records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "identity", table_name = "authorization_credentials")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationCredential {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field]
	pub token_hash: Vec<u8>,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub revoked_at: Option<DateTime<Utc>>,
	#[field(field_type = "text")]
	pub issued_by: String,
}

use crate::apps::identity::serializers::identity::Credential;
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::{DatabaseConnection, Model, OrmExecutor};

impl AuthorizationCredential {
	pub(crate) fn contract(self) -> Credential {
		Credential {
			id: self.id,
			tenant: self.tenant,
			subject: self.subject,
			created_at: self.created_at,
			expires_at: self.expires_at,
			revoked_at: self.revoked_at,
			issued_by: self.issued_by,
		}
	}

	pub(crate) async fn visible_page<E: OrmExecutor>(
		db: &mut E,
		tenant: &str,
		offset: u64,
	) -> Result<Vec<Credential>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.filter(Self::field_issued_by().ne("dashboard-oidc"))
			.order_by(&["-created_at", "id"])
			.limit(200)
			.offset(offset as usize)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(Self::contract)
			.collect())
	}

	pub(crate) async fn revoke(
		db: DatabaseConnection,
		tenant: &str,
		id: Uuid,
	) -> Result<Credential> {
		db.atomic(async |tx| {
			super::authority::authority_control(tx).await?;
			let row = Self::objects()
				.filter(Self::field_id().eq(id))
				.filter(Self::field_tenant().eq(tenant))
				.filter(Self::field_issued_by().ne("dashboard-oidc"))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?;
			let mut row = row
				.into_iter()
				.next()
				.ok_or_else(|| Error::NotFound("credential".into()))?;
			if row.revoked_at.is_none() {
				row.revoked_at = Some(Utc::now());
				Self::objects().update_with_conn(tx, &row).await?;
			}
			Ok(row.contract())
		})
		.await
	}
}

impl AuthorizationCredential {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
