//! Federation token identifiers a receiving Node has seen (ADR 0064, #989).
//!
//! A receiving Node in another Tenant records the sending Deployment and the
//! `jti` of every token whose signature verified, until the token's `exp` plus
//! 30 seconds, so a captured token cannot be replayed.

use crate::Result;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "federation", table_name = "federation_token_ids")]
#[derive(Serialize, Deserialize)]
pub struct FederationTokenId {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub node: String,
	/// The sending Deployment, the token's `iss`.
	#[field(primary_key = true, field_type = "text")]
	pub deployment: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub jti: Uuid,
	#[field]
	pub expires_at: DateTime<Utc>,
}

impl FederationTokenId {
	/// Records `jti` for the receiving `node`, first deleting the Node's expired
	/// rows. `false` means the key was already recorded.
	///
	/// The delete runs on the insert path, so no worker is needed and the table
	/// holds only what arrived since the Node last verified a token: tokens live
	/// 60 seconds and a record 30 seconds more.
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		node: &str,
		deployment: &str,
		jti: Uuid,
		expires_at: DateTime<Utc>,
	) -> Result<bool> {
		let table = Alias::new(Self::table_name());
		let (sql, values) = Query::delete()
			.from_table(table.clone())
			.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
			.and_where(Expr::col("node").eq(Expr::value(node.to_owned())))
			.and_where(Expr::col("expires_at").lt(Expr::current_timestamp()))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		let (sql, values) = Query::insert()
			.into_table(table)
			.columns(["tenant", "node", "deployment", "jti", "expires_at"].map(Alias::new))
			.values_panic([
				Expr::value(tenant.to_owned()),
				Expr::value(node.to_owned()),
				Expr::value(deployment.to_owned()),
				Expr::value(jti),
				Expr::value(expires_at),
			])
			.on_conflict(
				OnConflict::columns(["tenant", "node", "deployment", "jti"].map(Alias::new))
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			== 1)
	}
}
