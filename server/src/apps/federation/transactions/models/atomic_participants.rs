//! Persistent atomic_participants records.

use crate::apps::federation::transactions::services::states::AtomicParticipantPhase;
use crate::{Result, apps::federation::transactions::serializers::contracts::LocalStatus};
use chrono::{DateTime, Utc};
use reinhardt::db::orm::{Json, Model, OrmExecutor};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "federation", table_name = "atomic_participants")]
#[derive(Serialize, Deserialize)]
pub struct AtomicParticipant {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub coordinator: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub manifest: Json<Value>,
	#[field(field_type = "text", max_length = 64)]
	pub phase: AtomicParticipantPhase,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}

impl AtomicParticipant {
	pub(crate) async fn page<E: OrmExecutor>(db: &mut E) -> Result<Vec<LocalStatus>> {
		Ok(Self::objects()
			.all()
			.order_by(&["-updated_at", "id"])
			.limit(200)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(Into::into)
			.collect())
	}
}
