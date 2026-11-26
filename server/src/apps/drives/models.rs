//! Reinhardt records for Drive metadata and background work.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "execution", table_name = "drives")]
#[derive(Serialize, Deserialize)]
pub struct DrivesRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field]
	pub closed: bool,
	#[field]
	pub head: i64,
}

#[model(app_label = "execution", table_name = "drive_versions")]
#[derive(Serialize, Deserialize)]
pub struct DriveVersionsRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub drive_id: Uuid,
	#[field]
	pub version: i64,
	#[field(field_type = "text")]
	pub author: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub change_count: i64,
	#[field(null = true)]
	pub task_id: Option<Uuid>,
	#[field(null = true)]
	pub run_id: Option<Uuid>,
	#[field(null = true)]
	pub operation_id: Option<Uuid>,
}

#[model(app_label = "execution", table_name = "drive_entries")]
#[derive(Serialize, Deserialize)]
pub struct DriveEntriesRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub drive_id: Uuid,
	#[field(field_type = "text")]
	pub path: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub size: i64,
	#[field(field_type = "text")]
	pub media_type: String,
	#[field]
	pub added_in_version: i64,
	#[field(null = true)]
	pub removed_in_version: Option<i64>,
	#[field]
	pub indexed: bool,
}

#[model(app_label = "execution", table_name = "drive_trash")]
#[derive(Serialize, Deserialize)]
pub struct DriveTrashRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub drive_id: Uuid,
	#[field(field_type = "text")]
	pub path: String,
	#[field]
	pub entry_id: Uuid,
	#[field]
	pub removed_in_version: i64,
	#[field]
	pub expires_at: DateTime<Utc>,
}

#[model(app_label = "execution", table_name = "drive_uploads")]
#[derive(Serialize, Deserialize)]
pub struct DriveUploadsRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub drive_id: Uuid,
	#[field(field_type = "text")]
	pub author: String,
	#[field]
	pub spec: Json<Value>,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub published_version: Option<i64>,
}

#[model(app_label = "execution", table_name = "drive_upload_parts")]
#[derive(Serialize, Deserialize)]
pub struct DriveUploadPartsRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub upload_id: Uuid,
	#[field]
	pub number: i64,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub size: i64,
}

#[model(app_label = "execution", table_name = "drive_jobs")]
#[derive(Serialize, Deserialize)]
pub struct DriveJobsRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub drive_id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub task_id: Uuid,
	#[field(field_type = "text")]
	pub author: String,
	#[field]
	pub plan: Json<Value>,
	#[field(field_type = "text")]
	pub state: String,
	#[field(field_type = "text", null = true)]
	pub result: Option<String>,
	#[field]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "execution", table_name = "drive_job_blobs")]
#[derive(Serialize, Deserialize)]
pub struct DriveJobBlobsRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub job_id: Uuid,
	#[field(field_type = "text")]
	pub digest: String,
}

#[model(app_label = "execution", table_name = "drive_run_reads")]
#[derive(Serialize, Deserialize)]
pub struct DriveRunReadsRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub thread_id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub entry_id: Uuid,
}

#[model(app_label = "knowledge", table_name = "drive_index_requests")]
#[derive(Serialize, Deserialize)]
pub struct DriveIndexRequestsRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub drive_id: Uuid,
	#[field]
	pub version: i64,
	#[field(field_type = "text")]
	pub requester: String,
	#[field(null = true)]
	pub estimated_micros: Option<i64>,
	#[field(field_type = "text")]
	pub state: String,
}

#[model(app_label = "knowledge", table_name = "drive_passages")]
#[derive(Serialize, Deserialize)]
pub struct DrivePassagesRow {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(field_type = "text")]
	pub embedding_key: String,
	#[field]
	pub ordinal: i64,
	#[field(null = true)]
	pub start_line: Option<i64>,
	#[field(null = true)]
	pub end_line: Option<i64>,
	#[field(field_type = "text", null = true)]
	pub body: Option<String>,
	#[field]
	pub vector: Json<Value>,
}
