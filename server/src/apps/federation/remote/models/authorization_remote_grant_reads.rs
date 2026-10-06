//! Persistent authorization_remote_grant_reads records.

use crate::Result;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::model;
use reinhardt::query::{Alias, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(
	app_label = "federation",
	table_name = "authorization_remote_grant_reads"
)]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRemoteGrantRead {
	#[field(primary_key = true, field_type = "uuid")]
	pub grant_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "uuid")]
	pub workspace_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub resource_kind: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub resource_id: uuid::Uuid,
}

impl AuthorizationRemoteGrantRead {
	pub(crate) async fn record_sources(
		tx: &mut dyn TransactionExecutor,
		grant: Uuid,
		workspace: Uuid,
		sources: &[(String, Uuid)],
	) -> Result<()> {
		for chunk in sources.chunks(256) {
			let mut query = Query::insert();
			query
				.into_table(Alias::new(Self::table_name()))
				.columns(
					["grant_id", "workspace_id", "resource_kind", "resource_id"].map(Alias::new),
				)
				.on_conflict(
					OnConflict::columns([
						"grant_id",
						"workspace_id",
						"resource_kind",
						"resource_id",
					])
					.do_nothing(),
				);
			for (kind, id) in chunk {
				query.values_panic([
					IntoValue::into_value(grant),
					IntoValue::into_value(workspace),
					IntoValue::into_value(kind),
					IntoValue::into_value(*id),
				]);
			}
			let (sql, values) = query.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		Ok(())
	}
}

use reinhardt::query::IntoValue;

impl AuthorizationRemoteGrantRead {}
