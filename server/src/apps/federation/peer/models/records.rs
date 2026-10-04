//! Native peer records and the serialization boundary for credential assignment.
use super::Peer;
use crate::{Error, Result, federation::Peer as PeerContract};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{AtomicTransaction, Model, OrmExecutor};
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};

pub(crate) async fn list<E: OrmExecutor>(db: &mut E) -> Result<Vec<PeerContract>> {
	Ok(Peer::objects()
		.all()
		.order_by(&["node_id"])
		.all_with_db(db)
		.await?
		.into_iter()
		.map(PeerContract::from)
		.collect())
}

pub(crate) async fn enabled<E: OrmExecutor>(db: &mut E, node: &str) -> Result<PeerContract> {
	Peer::objects()
		.filter(Peer::field_node_id().eq(node))
		.filter(Peer::field_enabled().eq(true))
		.first_with_db(db)
		.await?
		.map(PeerContract::from)
		.ok_or(Error::Unauthorized)
}

/// Every enabled registration takes the same transaction lock so concurrent
/// registrations cannot assign one secret to different node identities.
pub(crate) async fn lock_registration(tx: &mut AtomicTransaction) -> Result<()> {
	let query = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![Expr::value(71003203_i64).into()],
		))
		.to_string(PostgresQueryBuilder);
	TransactionExecutor::execute(tx, &query, vec![]).await?;
	Ok(())
}

pub(crate) async fn disable(tx: &mut AtomicTransaction, node: &str) -> Result<PeerContract> {
	let mut stored = Peer::objects()
		.filter(Peer::field_node_id().eq(node))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.ok_or_else(|| Error::NotFound("peer".into()))?;
	stored.enabled = false;
	Ok(Peer::objects().update_with_conn(tx, &stored).await?.into())
}

/// Called under the registration lock; existing rows retain their row locks
/// until the event and the configuration commit together.
pub(crate) async fn save(tx: &mut AtomicTransaction, peer: &PeerContract) -> Result<()> {
	let existing = Peer::objects()
		.filter(Peer::field_node_id().eq(peer.node_id.clone()))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?;
	let record = Peer::build()
		.node_id(&peer.node_id)
		.endpoint(&peer.endpoint)
		.credential_env(&peer.credential_env)
		.protocol_version(&peer.protocol_version)
		.enabled(peer.enabled)
		.finish();
	if existing.is_empty() {
		Peer::objects().create_with_conn(tx, &record).await?;
	} else {
		Peer::objects().update_with_conn(tx, &record).await?;
	}
	Ok(())
}

/// Restore peer authentication while preserving disabled subject mappings/trust.
pub(crate) async fn restore_authentication(
	db: reinhardt::db::orm::DatabaseConnection,
	node: &str,
	credential_env: &str,
	credential: &str,
) -> Result<PeerContract> {
	db.atomic(async |tx| {
		crate::apps::identity::models::authority_control(tx).await?;
		lock_registration(tx).await?;
		for other in list(tx)
			.await?
			.into_iter()
			.filter(|other| other.enabled && other.node_id != node)
		{
			if crate::config::peer_secret(&other.credential_env)? == credential {
				return Err(Error::Invalid(
					"enabled peers must use distinct credentials for each node identity".into(),
				));
			}
		}
		let mut peer = Peer::objects()
			.filter(Peer::field_node_id().eq(node))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or_else(|| Error::NotFound("existing peer".into()))?;
		peer.credential_env = credential_env.to_owned();
		peer.enabled = true;
		let peer = Peer::objects().update_with_conn(tx, &peer).await?;
		crate::apps::federation::transactions::models::AtomicHistory::append(
			tx,
			uuid::Uuid::nil(),
			"trust",
			"AUTHENTICATION_RESTORED",
			node,
		)
		.await?;
		Ok(peer.into())
	})
	.await
}
