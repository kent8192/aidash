//! Persistent peers records.

use crate::{Result, federation::Peer as PeerContract};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "federation", table_name = "peers")]
#[derive(Serialize, Deserialize)]
pub struct Peer {
	#[field(primary_key = true, field_type = "text")]
	pub node_id: String,
	#[field(field_type = "text")]
	pub endpoint: String,
	#[field(field_type = "text")]
	pub credential_env: String,
	#[field(field_type = "text")]
	pub protocol_version: String,
	#[field(default = true)]
	pub enabled: bool,
}

impl Peer {
	pub(crate) async fn enabled_in(
		tx: &mut dyn TransactionExecutor,
		node: &str,
	) -> Result<Option<PeerContract>> {
		use reinhardt::db::orm::execution::convert_values;
		use reinhardt::db::orm::{Model, QueryRow};
		use reinhardt::query::{
			Alias, ColumnRef, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query,
			QueryStatementBuilder,
		};
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("node_id").eq(Expr::value(node)))
			.and_where(Expr::col("enabled").eq(Expr::value(true)))
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
			.transpose()?)
	}
}
