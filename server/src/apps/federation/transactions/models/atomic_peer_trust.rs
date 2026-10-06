//! Persistent atomic_peer_trust records.

use super::{AtomicHistory, coordinator_records::advisory_lock};
use crate::Result;
use crate::apps::federation::peer::models::Peer;
use crate::apps::federation::transactions::serializers::protocol::TransactionTrust;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{DatabaseConnection, Model, OrmExecutor};
use reinhardt::model;
use reinhardt::query::{PostgresQueryBuilder, Query, QueryStatementBuilder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "federation", table_name = "atomic_peer_trust")]
#[derive(Serialize, Deserialize)]
pub struct AtomicPeerTrust {
	#[field(field_type = "text", primary_key = true)]
	pub node_id: String,
	#[field]
	pub enabled: bool,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}

impl AtomicPeerTrust {
	pub(crate) async fn page<E: OrmExecutor>(db: &mut E) -> Result<Vec<TransactionTrust>> {
		Ok(Self::objects()
			.all()
			.order_by(&["node_id"])
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|row| TransactionTrust {
				node_id: row.node_id,
				enabled: row.enabled,
			})
			.collect())
	}

	pub(crate) async fn set(
		connection: DatabaseConnection,
		input: TransactionTrust,
	) -> Result<TransactionTrust> {
		connection
			.atomic(async |tx| {
				let (sql, values) = Query::select()
					.expr(advisory_lock(
						"pg_advisory_xact_lock",
						format!("atomic:trust:{}", input.node_id),
					))
					.build(PostgresQueryBuilder);
				TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
				if let Some(mut row) = Self::objects()
					.filter(Self::field_node_id().eq(input.node_id.clone()))
					.first_with_db(tx)
					.await?
				{
					row.enabled = input.enabled;
					row.updated_at = Utc::now();
					Self::objects().update_with_conn(tx, &row).await?;
				} else {
					let peer = Peer::objects()
						.filter(Peer::field_node_id().eq(input.node_id.clone()))
						.get_with_db(tx)
						.await?;
					let row = Self::build()
						.node_id(&peer.node_id)
						.enabled(input.enabled)
						.finish();
					Self::objects().create_with_conn(tx, &row).await?;
				}
				AtomicHistory::append(
					tx,
					Uuid::nil(),
					"trust",
					if input.enabled { "ENABLED" } else { "DISABLED" },
					&input.node_id,
				)
				.await?;
				Ok(input)
			})
			.await
	}
}

impl AtomicPeerTrust {}
