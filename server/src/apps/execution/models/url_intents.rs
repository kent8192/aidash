//! URL intents: the pages a person asked one Run to read, recorded beside the
//! input that carried them and never in its text (ADR 0052). A row with no
//! `input_key` belongs to the Run's Task; any other names one run input.
use crate::Result;
use aidash_domain::research::intent::UrlIntent;
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Json, Model};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "execution", table_name = "url_intents")]
#[derive(Serialize, Deserialize)]
pub struct UrlIntentRecord {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "url_intents_run",
		db_index = false,
		on_delete = Cascade,
		on_update = Restrict
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text", null = true)]
	pub input_key: Option<String>,
	#[field]
	pub pages: Json<Vec<String>>,
	#[field(field_type = "text")]
	pub requested_by: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl UrlIntentRecord {
	/// Record what `requested_by` asked `run` to read with one input. A
	/// replayed input whose intent is already recorded records nothing more.
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		input_key: Option<&str>,
		pages: &[String],
		requested_by: &str,
	) -> Result<()> {
		if pages.is_empty() {
			return Ok(());
		}
		let existing = Self::objects()
			.filter(Self::field_run_id().eq(run))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?;
		if existing
			.iter()
			.any(|record| record.input_key.as_deref() == input_key)
		{
			return Ok(());
		}
		let tenant = crate::database::scope::parent_tenant(tx, "runs", "id", run).await?;
		let record = Self::build()
			.tenant(tenant)
			.id(Uuid::now_v7())
			.run(run)
			.input_key(input_key.map(str::to_owned))
			.pages(Json(pages.to_vec()))
			.requested_by(requested_by.to_owned())
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}

	/// Every URL intent of `run`, oldest first.
	pub(crate) async fn for_run(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<Vec<Self>> {
		let mut records = Self::objects()
			.filter(Self::field_run_id().eq(run))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?;
		records.sort_by_key(|record| (record.created_at, record.id));
		Ok(records)
	}

	pub(crate) fn intent(&self) -> UrlIntent {
		UrlIntent {
			input_key: self.input_key.clone(),
			pages: self.pages.0.clone(),
		}
	}
}
