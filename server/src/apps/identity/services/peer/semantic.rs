//! Leaf receiver checks. These handlers do not call Home or acquire its locks.
use crate::semantic::remote::{Boundary, InputRead, Receipt, bounded_query};
use crate::{
	Error, Result,
	federation::Federation,
	semantic::remote::{Failure, Operation, journal},
};
use aidash_application::tenancy::TenancyScope;

use reinhardt::query::{Alias, Expr, OnConflict, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use uuid::Uuid;

/// The receiver's pinned admission Description for a remote Run.
/// `tenancy` is the receiver's mapped Tenant of the Run.
pub(crate) async fn admission(
	f: &Federation,
	run: &crate::domain::Run,
	tenancy: &TenancyScope,
) -> Result<crate::authorization::remote::Description> {
	let description: Value = {
		let query_bind_1 = run.id;
		let query_bind_2 = &run.home_node;
		crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("description"))
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=? AND source_node=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(f.store.pool.in_scope(tenancy))
		.await?
	}
	.ok_or(Error::Forbidden)?;
	Ok(serde_json::from_value(description)?)
}

pub(crate) async fn context(
	f: &Federation,
	run: &crate::domain::Run,
	tenancy: &TenancyScope,
	task: &crate::domain::Task,
	inputs: &[(InputRead, String)],
	query: &str,
	budget: usize,
) -> Result<Option<Value>> {
	let description = admission(f, run, tenancy).await?;
	if description.semantic.disabled() {
		return Ok(None);
	}
	if budget == 0 {
		return Err(Error::RemoteSemantic(Failure::ContextBudget));
	}
	let refs = inputs
		.iter()
		.map(|(read, _)| read.clone())
		.collect::<Vec<_>>();
	let mut operation = Operation {
		id: Uuid::nil(),
		home_node: run.home_node.clone(),
		grant_id: description.grant_id,
		admission_id: run.id,
		boundary: Boundary {
			step: run.step,
			input_sequence: refs
				.last()
				.map_or(run.observed_input_seq, |read| read.sequence),
			task_revision: task.revision,
			inputs_digest: crate::registry::digest(&json!(refs)),
		},
		inputs: refs,
		query: bounded_query(query, 32768).0,
		max_tokens: budget.min(32768),
		metadata: json!({}),
	};
	operation.set_id()?;
	let prepared = journal::prepare(&f.store, tenancy, &operation, &description.semantic).await?;
	let receipt: Receipt = super::authority_request(
		f,
		&aidash_domain::federation::route::Sender {
			node: Some(run.node.clone()),
			..super::scope_sender(tenancy)?
		},
		&run.home_node,
		"/scoped/semantic/query",
		&serde_json::to_value(&operation)?,
	)
	.await
	.map_err(|error| {
		Error::RemoteSemantic(crate::authorization::remote::semantic::failure(&error))
	})?;
	receipt.validate_native()?;
	if receipt.operation_id != operation.id
		|| receipt.operation_digest != operation.digest()?
		|| receipt.home_node != run.home_node
		|| receipt.tenant != description.source_tenant
		|| receipt.workspace_id != run.workspace_id
		|| receipt.grant_id != description.grant_id
		|| receipt.admission_id != run.id
		|| receipt.binding != description.semantic
		|| receipt.executor != run.executor_subject(f.config.deployment.as_str())
		|| receipt.sources.len() != receipt.result.matches.len()
		|| receipt
			.sources
			.iter()
			.zip(&receipt.result.matches)
			.any(|(source, m)| {
				source.entry_id != m.entry_id
					|| source.revision != m.revision
					|| source.content_digest != crate::semantic::service::content_digest(&m.text)
			}) || (if receipt.binding.native().is_some() {
		serde_json::to_vec(&receipt)?.len()
	} else {
		crate::context::estimated_tokens(&serde_json::to_string(&receipt)?)
	}) > operation.max_tokens
	{
		return Err(Error::RemoteSemantic(Failure::ProviderContract));
	}
	let mut tx = f.store.pool.begin(tenancy).await?;
	let value = persist_reply_in(
		&f.store,
		&mut tx,
		run.id,
		&operation,
		&receipt,
		prepared.fence,
	)
	.await?;
	tx.commit().await?;
	Ok(Some(value))
}

/// Persist only while this operation still owns its reply allowance. Source
/// invalidation locks this same row and advances its fence before clearing body
/// copies, so a preauthorized late reply cannot resurrect erased quotations.
pub(crate) async fn persist_reply_in(
	store: &crate::store::Store,
	tx: &mut crate::database::native::Transaction,
	run: Uuid,
	operation: &Operation,
	receipt: &Receipt,
	prepared_fence: i64,
) -> Result<Value> {
	use crate::database::native;
	use reinhardt::query::{ExprTrait, LockType};
	let tenant = crate::database::scope::parent_tenant(&mut **tx, "runs", "id", run).await?;
	let digest = operation.digest()?;
	if receipt.operation_id != operation.id
		|| receipt.operation_digest != digest
		|| receipt.home_node != operation.home_node
		|| receipt.grant_id != operation.grant_id
		|| receipt.admission_id != run
		|| operation.admission_id != run
	{
		return Err(Error::RemoteSemantic(Failure::ProviderContract));
	}
	let value = serde_json::to_value(receipt)?;
	let row = native::query(
		&Query::select()
			.columns(["state", "fence", "receipt", "error"].map(Alias::new))
			.from(Alias::new("semantic_remote_operations"))
			.and_where(Expr::col("tenant").eq(Expr::value(&tenant)))
			.and_where(Expr::col("id").eq(Expr::value(operation.id)))
			.and_where(Expr::col("grant_id").eq(Expr::value(operation.grant_id)))
			.and_where(Expr::col("admission_id").eq(Expr::value(run)))
			.and_where(Expr::col("home_node").eq(Expr::value(&operation.home_node)))
			.and_where(Expr::col("digest").eq(Expr::value(&digest)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **tx)
	.await?
	.ok_or(Error::RemoteSemantic(Failure::Invalidated))?;
	let state: String = row.try_get("state")?;
	let fence: i64 = row.try_get("fence")?;
	match state.as_str() {
		"INVALIDATED" => return Err(Error::RemoteSemantic(Failure::Invalidated)),
		"CANCELLED" => return Err(Error::RemoteSemantic(Failure::Authority)),
		"PAUSED" => {
			let error: Option<String> = row.try_get("error")?;
			return Err(Error::RemoteSemantic(
				aidash_domain::semantic::remote::journal::reason(error.as_deref()),
			));
		}
		"PENDING" if fence == prepared_fence => {}
		"READY" => {
			let cached: Option<Value> = row.try_get("receipt")?;
			if cached.as_ref() != Some(&value) {
				return Err(Error::RemoteSemantic(Failure::Invalidated));
			}
			if fence != prepared_fence {
				// A local Home claim advances the shared operation fence. Only
				// its already-completed exact reply with verified Home authority
				// can satisfy this exception; READY alone is never enough.
				let node: Option<String> = native::query_scalar(
					&Query::select()
						.column(Alias::new("node"))
						.from(Alias::new("authorization_remote_grants"))
						.and_where(Expr::col("tenant").eq(Expr::value(&tenant)))
						.and_where(Expr::col("id").eq(Expr::value(operation.grant_id)))
						.lock(LockType::KeyShare)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **tx)
				.await?;
				let home = node
					.and_then(|node| aidash_domain::nodes::NodeName::parse(&node).ok())
					.map(|node| store.deployment.node(node).to_string());
				if home.as_deref() != Some(operation.home_node.as_str()) {
					return Err(Error::RemoteSemantic(Failure::Invalidated));
				}
			}
		}
		_ => return Err(Error::RemoteSemantic(Failure::Pending)),
	}

	crate::apps::knowledge::repositories::receiver_caches::record(tx, run, receipt).await?;
	native::query(
		&Query::insert()
			.into_table(Alias::new("semantic_remote_receipts"))
			.columns(["operation_id", "run_id", "digest", "receipt", "tenant"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(operation.id))
					.expr(Expr::value(run))
					.expr(Expr::value(&digest))
					.expr(Expr::value(value.clone()))
					.expr(Expr::value(&tenant))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("operation_id"))
					.update_columns([Alias::new("receipt")])
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	native::query(
		&Query::update()
			.table(Alias::new("semantic_remote_operations"))
			.value(Alias::new("state"), "READY")
			.value(Alias::new("receipt"), value.clone())
			.and_where(Expr::col("tenant").eq(Expr::value(&tenant)))
			.and_where(Expr::col("id").eq(Expr::value(operation.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(value)
}

pub(crate) async fn verify_operation(
	f: Federation,
	admitted: Admitted,
	input: Operation,
) -> Result<bool> {
	let source = admitted.sender.to_string();
	if input.home_node != source {
		return Err(Error::Forbidden);
	}
	input.validate()?;
	let (access, description) = super::admission::leaf_lease(
		&f,
		&admitted.receiver.node,
		&source,
		input.grant_id,
		input.admission_id,
	)
	.await?;
	let result = async {
		if description.semantic.disabled() {
			return Err(Error::RemoteSemantic(Failure::Configuration));
		}
		verify_in(&f, &description, &input).await?;
		Ok(true)
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn verify_in(
	f: &Federation,
	description: &crate::authorization::remote::Description,
	input: &Operation,
) -> Result<()> {
	// The receiver durably fixed these inputs before asking Home. Home cannot
	// substitute a different query, step, budget, or admission in a callback.
	let scope = f.store.run_tenancy(input.admission_id).await?;
	let record = journal::bound(&f.store, &scope, input, &description.semantic).await?;
	if record.digest != input.digest()? {
		return Err(Error::Forbidden);
	}
	let run = f.store.run(&scope, input.admission_id).await?;
	if run.step != input.boundary.step {
		return Err(Error::Forbidden);
	}
	let inputs = f.store.run_inputs(&scope, run.id).await?;
	for required in &input.inputs {
		if !inputs.iter().any(|i| {
			i.seq == required.sequence
				&& i.message_id == Some(required.id)
				&& crate::semantic::service::content_digest(&i.content) == required.digest
		}) {
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}

use reinhardt::query::QueryStatementBuilder as _;

use aidash_application::federation::inbound::Admitted;

use reinhardt::query::SimpleExpr;
