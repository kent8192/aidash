//! Persistent core_requests records.
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "execution", table_name = "core_requests")]
#[derive(Serialize, Deserialize)]
pub struct CoreRequest {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub principal: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub key: uuid::Uuid,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub result: Json<Value>,
}

use crate::Result;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
impl CoreRequest {
	pub(crate) async fn remember(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		principal: &str,
		key: Uuid,
		digest: &str,
		result: Value,
	) -> Result<()> {
		let record = Self::build()
			.tenant(tenant)
			.principal(principal)
			.key(key)
			.digest(digest)
			.result(Json(result))
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}
}
