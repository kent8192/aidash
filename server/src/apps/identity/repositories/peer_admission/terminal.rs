//! The real response and purge share this transaction-local projection owner.
use super::*;
use crate::{Error, Result};
use aidash_domain::{
	federation::execution::admission::{
		Activation, RemoteExecutionControlState as Control, RemoteExecutionPhase as Phase,
	},
	nodes::{DeploymentId, NodeAddress},
};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{OnConflict, Order};
const TABLE: &str = "authorization_remote_terminal_receipts";
const SANITIZED_ERROR: &str = "Remote execution requires attention.";
struct Receipt {
	id: Uuid,
	tenant: String,
	node: String,
	source_node: String,
	grant_id: Uuid,
	activation: Value,
}
crate::native_record!(Receipt {
	id,
	tenant,
	node,
	source_node,
	grant_id,
	activation
});
fn blocked() -> Error {
	Error::Conflict("Node purge cannot verify terminal execution recovery evidence; retain the original admission and receiving trust".into())
}
fn terminal(value: &Activation) -> bool {
	matches!(
		value.phase,
		Phase::Completed | Phase::Failed | Phase::Cancelled
	) && !matches!(value.control, Control::Inactive)
		|| (matches!(value.phase, Phase::Admitted)
			&& matches!(value.control, Control::Cancelled)
			&& value.semantic_reason.is_none()
			&& value.context_reason.is_none()
			&& value.error.is_none())
}
fn checked(receipt: &Receipt) -> Result<Activation> {
	let activation: Activation =
		serde_json::from_value(receipt.activation.clone()).map_err(|_| blocked())?;
	aidash_domain::tenancy::TenantName::new(&receipt.tenant).map_err(|_| blocked())?;
	if NodeAddress::parse(&receipt.source_node)
		.map_err(|_| blocked())?
		.to_string()
		!= receipt.source_node
	{
		return Err(blocked());
	}
	stored_name(&receipt.node)?;
	if activation.admission_id != receipt.id
		|| activation.run_id != receipt.id
		|| activation.grant_id != receipt.grant_id
		|| !terminal(&activation)
		|| activation
			.error
			.as_deref()
			.is_some_and(|error| error != SANITIZED_ERROR)
		|| serde_json::to_value(&activation)? != receipt.activation
	{
		return Err(blocked());
	}
	Ok(activation)
}
fn stored_name(node: &str) -> Result<aidash_domain::nodes::NodeName> {
	aidash_domain::nodes::NodeName::parse(node).map_err(|_| blocked())
}
async fn receipt(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
	tenant: &str,
) -> Result<Option<Receipt>> {
	crate::database::native::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new(TABLE))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("tenant").eq(tenant))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(tx)
	.await
}
/// Caller holds current Tenant policy before capacity/receiving-owner locks.
/// No catalog or deletion-history fallback; permanent names prove ownership.
pub(crate) async fn replay_in(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	receiver: &NodeAddress,
	source: &NodeAddress,
	subject: &str,
	id: Uuid,
	grant: Uuid,
	request: &aidash_application::federation::inbound::RequestDigest<'_>,
) -> Result<Option<Activation>> {
	if crate::apps::identity::serializers::peer_admission::cancel_request(request)
		!= Some((id, grant))
		|| !subject.is_empty()
		|| super::super::tenant_deletions::is_deleted(tx, tenant).await?
		|| super::super::node_registrations::name_tenant(tx, receiver.node.as_str())
			.await?
			.as_deref()
			!= Some(tenant)
	{
		return Ok(None);
	}
	match crate::apps::identity::models::AuthorizationBundle::lock_snapshot(tx, tenant, false).await
	{
		Err(Error::NotFound(_) | Error::TenantDeleted) => return Ok(None),
		other => {
			other?;
		}
	}
	let Some(saved) = receipt(tx, id, tenant).await? else {
		return Ok(None);
	};
	let activation = checked(&saved)?;
	if saved.node != receiver.node.as_str()
		|| saved.source_node != source.to_string()
		|| saved.grant_id != grant
	{
		return Ok(None);
	}
	Ok(Some(activation))
}
/// Existing evidence must agree with the same locked authoritative projection
/// both before physical unlink and when the response/Rows owner persists it.
fn agrees(saved: &Receipt, record: &Record, activation: &Activation) -> Result<()> {
	checked(saved)?;
	if saved.id != record.id
		|| saved.tenant != record.tenant
		|| saved.node != record.node
		|| saved.source_node != record.source_node
		|| saved.grant_id != record.grant_id
		|| saved.activation != serde_json::to_value(activation)?
	{
		return Err(blocked());
	}
	Ok(())
}
async fn persist(
	tx: &mut dyn TransactionExecutor,
	record: &Record,
	activation: &Activation,
) -> Result<()> {
	let value = serde_json::to_value(activation)?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new(TABLE))
		.columns(
			[
				"id",
				"tenant",
				"node",
				"source_node",
				"grant_id",
				"activation",
			]
			.map(Alias::new),
		)
		.values_panic([
			Expr::value(record.id),
			Expr::value(record.tenant.clone()),
			Expr::value(record.node.clone()),
			Expr::value(record.source_node.clone()),
			Expr::value(record.grant_id),
			Expr::value(value.clone()),
		])
		.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
		.build(PostgresQueryBuilder);
	tx.execute(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await?;
	let saved = receipt(tx, record.id, &record.tenant)
		.await?
		.ok_or_else(blocked)?;
	agrees(&saved, record, activation)
}
/// Project from the exact locked admission and its actual Run, never a cached response.
/// A terminal projection is compact; output/context/description are not retained.
async fn project(
	tx: &mut dyn TransactionExecutor,
	deployment: &DeploymentId,
	record: &Record,
) -> Result<Activation> {
	let source = NodeAddress::parse(&record.source_node).map_err(|_| blocked())?;
	let receiver = deployment.node(stored_name(&record.node)?);
	let description: Description =
		serde_json::from_value(record.description.clone()).map_err(|_| blocked())?;
	if source.to_string() != record.source_node
		|| source == receiver
		|| description.grant_id != record.grant_id
		|| description.source_node != record.source_node
		|| description.target_node != receiver.to_string()
		|| description.task.id != record.task_id
	{
		return Err(blocked());
	}
	let raw: Option<aidash_domain::run_state::RawRun> = crate::database::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(record.id)))
			.and_where(Expr::col("tenant").eq(record.tenant.clone()))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(tx)
	.await?;
	let (phase, control, semantic_reason, context_reason, error) = if let Some(raw) = raw {
		let state: State = Record {
			cancelled: record.cancelled,
			id: record.id,
			node: record.node.clone(),
			source_node: record.source_node.clone(),
			grant_id: record.grant_id,
			task_id: record.task_id,
			tenant: record.tenant.clone(),
			credential_id: record.credential_id,
			subject_chain: record.subject_chain.clone(),
			description: record.description.clone(),
		}
		.into();
		if raw.metadata.node != receiver.node || !state.matches_run(&raw.metadata, &description) {
			return Err(blocked());
		}
		let run = raw.inspect();
		(
			run.phase().into(),
			run.control.into(),
			run.recovery.as_ref().and_then(|r| r.semantic_reason),
			run.recovery.as_ref().and_then(|r| r.context_reason),
			run.error.as_ref().map(|_| SANITIZED_ERROR.to_owned()),
		)
	} else if record.cancelled {
		(Phase::Admitted, Control::Cancelled, None, None, None)
	} else {
		return Err(blocked());
	};
	Ok(Activation {
		grant_id: record.grant_id,
		admission_id: record.id,
		run_id: record.id,
		phase,
		control,
		semantic_reason,
		context_reason,
		error,
	})
}
/// The response owner calls this before committing; purge uses the same projection.
pub(super) async fn response_in(
	tx: &mut dyn TransactionExecutor,
	deployment: &DeploymentId,
	record: &Record,
) -> Result<Activation> {
	let activation = project(tx, deployment, record).await?;
	if terminal(&activation) {
		persist(tx, record, &activation).await?;
	}
	Ok(activation)
}
/// Preflight validates every planned receipt before files; Rows captures first,
/// before registration dependencies and the destructive CTID snapshot are computed.
pub(crate) async fn purge_in(
	tx: &mut dyn TransactionExecutor,
	deployment: &DeploymentId,
	tenant: &str,
	node: &str,
	capture: bool,
) -> Result<Vec<String>> {
	let records: Vec<Record> = crate::database::native::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("node").eq(node))
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	let mut sources = Vec::new();
	for record in records {
		let activation = project(tx, deployment, &record).await?;
		if !terminal(&activation) {
			return Err(blocked());
		}
		if let Some(saved) = receipt(tx, record.id, &record.tenant).await? {
			agrees(&saved, &record, &activation)?;
		}
		if capture {
			persist(tx, &record, &activation).await?;
		}
		sources.push(record.source_node);
	}
	Ok(sources)
}
/// Validate all retained evidence and return only its exact original Home sources.
pub(crate) async fn sources_in(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
) -> Result<Vec<String>> {
	let saved: Vec<Receipt> = crate::database::native::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new(TABLE))
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("node").eq(node))
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	saved
		.into_iter()
		.map(|r| {
			checked(&r)?;
			Ok(r.source_node)
		})
		.collect()
}
