// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Service-owned ORM model; shared browser wire data lives in serializers.rs.
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[model(
	app_label = "marketplace_distribution",
	table_name = "marketplace_documents"
)]
#[derive(Serialize, Deserialize)]
pub struct Document {
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field(field_type = "text")]
	pub kind: String,
	#[field]
	pub document: Json<Value>,
}
