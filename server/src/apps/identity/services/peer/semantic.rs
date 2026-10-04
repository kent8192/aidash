//! Leaf receiver checks. These handlers do not call Home or acquire its locks.
use crate::semantic::remote::{Boundary, InputRead, Receipt, bounded_query};
use crate::{
	Error, Result,
	federation::Federation,
	semantic::remote::{Failure, Operation, journal},
};

use reinhardt::query::{Alias, Expr, OnConflict, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) async fn context(
	f: &Federation,
	run: &crate::domain::Run,
	task: &crate::domain::Task,
	inputs: &[(InputRead, String)],
	query: &str,
	budget: usize,
) -> Result<Option<Value>> {
	let description: Value = {
		let query_bind_1 = run.id;
		let query_bind_2 = &run.home_node;
		sqlx::query_scalar(
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
		.fetch_optional(&f.store.pool)
		.await?
	}
	.ok_or(Error::Forbidden)?;
	let description: crate::authorization::remote::Description =
		serde_json::from_value(description)?;
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
	journal::prepare(&f.store, &operation, &description.semantic).await?;
	let receipt: Receipt = super::authority_request(
		f,
		&run.home_node,
		"/scoped/semantic/query",
		&serde_json::to_value(&operation)?,
	)
	.await
	.map_err(|error| {
		Error::RemoteSemantic(crate::authorization::remote::semantic::failure(&error))
	})?;
	if receipt.operation_id != operation.id
		|| receipt.operation_digest != operation.digest()?
		|| receipt.home_node != run.home_node
		|| receipt.tenant != description.source_tenant
		|| receipt.workspace_id != run.workspace_id
		|| receipt.grant_id != description.grant_id
		|| receipt.admission_id != run.id
		|| receipt.binding != description.semantic
		|| receipt.executor
			!= crate::domain::qualified_agent(&f.config.node_id, &run.agent_id, &run.agent_version)
		|| receipt.sources.len() != receipt.result.matches.len()
		|| receipt
			.sources
			.iter()
			.zip(&receipt.result.matches)
			.any(|(source, m)| {
				source.entry_id != m.entry_id
					|| source.revision != m.revision
					|| source.content_digest != crate::semantic::service::content_digest(&m.text)
			}) || crate::context::estimated_tokens(&serde_json::to_string(&receipt)?)
		> operation.max_tokens
	{
		return Err(Error::RemoteSemantic(Failure::ProviderContract));
	}
	let value = serde_json::to_value(&receipt)?;
	// Persist before this function makes the text available to compaction/model
	// construction. The Home keeps the accumulated dependencies across replays.
	let mut tx = f.store.pool.begin().await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("semantic_remote_receipts"))
			.columns(["operation_id", "run_id", "digest", "receipt"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::cust("$1"))
					.expr(Expr::cust("$2"))
					.expr(Expr::cust("$3"))
					.expr(Expr::cust("$4"))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("operation_id"))
					.update_columns([Alias::new("receipt")])
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(operation.id)
	.bind(run.id)
	.bind(operation.digest()?)
	.bind(&value)
	.execute(&mut *tx)
	.await?;
	{
		let query_bind_1 = operation.id;
		let query_bind_2 = &value;
		sqlx::query(
			&Query::update()
				.table(Alias::new("semantic_remote_operations"))
				.value(Alias::new("state"), "READY")
				.value_expr(
					Alias::new("receipt"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?
	};
	tx.commit().await?;
	Ok(Some(value))
}

pub(crate) async fn verify_operation(
	f: Federation,
	headers: HeaderMap,
	input: Operation,
) -> Result<bool> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	if input.home_node != source {
		return Err(Error::Forbidden);
	}
	input.validate()?;
	let (access, description) =
		super::admission::leaf_lease(&f, source, input.grant_id, input.admission_id).await?;
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
	let record = journal::bound(&f.store, input, &description.semantic).await?;
	if record.digest != input.digest()? {
		return Err(Error::Forbidden);
	}
	let run = f.store.run(input.admission_id).await?;
	if run.step != input.boundary.step {
		return Err(Error::Forbidden);
	}
	let inputs = f.store.run_inputs(run.id).await?;
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

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use http::HeaderMap;

use reinhardt::query::SimpleExpr;
