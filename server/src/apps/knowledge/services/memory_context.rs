//! Every inference receives bounded Home memory with exact role and source provenance.
use super::native_memory::{self as memory, Action, Operation, Outcome};
use crate::apps::knowledge::repositories::{access::Lease, bindings, native_memory as repository};
use crate::{
	Error, Result,
	domain::{Run, Task},
	registry::AgentConfig,
	store::Store,
};
use aidash_domain::registry::EntityRef;
use aidash_domain::{context::sources::RetrievalKey, memory::*, semantic::InputRead};
use serde_json::{Value, json};

/// Every declaration for the same exact bank/provider contributes its cap.
pub(super) fn declare_bank(
	declared: &mut Vec<(Bank, EntityRef, usize)>,
	bank: Bank,
	provider: EntityRef,
	tokens: usize,
) {
	if let Some((_, _, limit)) = declared
		.iter_mut()
		.find(|(other, bound, _)| other == &bank && bound == &provider)
	{
		*limit = (*limit).min(tokens);
	} else {
		declared.push((bank, provider, tokens));
	}
}

/// Leave enough room for the combined envelope and a complete minimal status.
/// Reserve both null placeholders conservatively before ordinary retrieval.
pub(crate) fn workspace_budget(budget: usize) -> Result<usize> {
	let framing =
		serde_json::to_vec(&json!({"workspace":null,"memory":{"status":"no_space"}}))?.len();
	Ok(budget.saturating_sub(framing))
}

pub(crate) struct Context {
	value: Option<Value>,
	delivered: Vec<Unit>,
}
impl Context {
	fn status(value: Option<Value>) -> Self {
		Self {
			value,
			delivered: Vec::new(),
		}
	}
}

/// `key` is present for an Ordered Run. Its boundary is then the Retrieval Key
/// itself, without step or Run revision, so a re-retrieval under the same key
/// replays the same recall operation and envelope bytes.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn retrieve(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &Run,
	task: &Task,
	inputs: &[(InputRead, String)],
	budget: usize,
	agent: &AgentConfig,
	key: Option<&RetrievalKey>,
) -> Result<Context> {
	if run.home_node != store.node_id {
		return Err(Error::Forbidden);
	}
	if agent.memory.is_none() || agent.allow_cross_conversation_memory == Some(false) {
		return Ok(Context::status(bounded_status("disabled", budget)?));
	}
	crate::apps::execution::services::task_evidence::record(lease.tx(), run, task).await?;
	let binding = bindings::load(&mut **lease.tx(), &run.metadata())
		.await?
		.ok_or_else(|| {
			Error::Conflict("enabled memory requires a Home-issued participant".into())
		})?;
	if agent.memory.as_ref() != Some(&binding.provider) {
		return Err(Error::Conflict("Run memory definition changed".into()));
	}
	let mut declared = vec![(binding.bank.clone(), binding.provider.clone(), budget)];
	for reference in &agent.sources {
		let entry = memory::definition(lease, reference, "source").await?;
		let source: SourceConfig = serde_json::from_value(entry.config.clone())?;
		if let Some(access) = lease.access() {
			access.track_registry(&[entry]).await?;
		}
		if source.max_tokens == 0 {
			return Err(Error::Invalid(
				"memory Source needs a finite token budget".into(),
			));
		}
		let mut bank = binding.bank.clone();
		if source.scope == SourceScope::Workspace {
			bank.participant = None;
		}
		declare_bank(&mut declared, bank, source.memory, source.max_tokens);
	}
	let mut query = format!("{}\n{}", task.title, task.description);
	for (_, text) in inputs {
		query.push('\n');
		query.push_str(text);
	}
	let boundary = match key {
		Some(key) => serde_json::to_value(key)?,
		None => {
			json!({"task_revision":task.revision,"inputs":inputs.iter().map(|(read,_)|read).collect::<Vec<_>>(),"step":run.step,"run_revision":run.revision})
		}
	};
	let mut delivered = Vec::new();
	let mut envelope =
		json!({"home":store.node_id,"binding":binding,"boundary":boundary,"banks":[]});
	// UTF-8 bytes are the declared conservative tokenizer's token upper bound.
	// Count wrappers, identity, roles and complete source envelopes as well.
	for (ordinal, (bank, provider, declared_tokens)) in declared.into_iter().enumerate() {
		memory::scope(store, lease, &bank, "memory.read").await?;
		memory::bank_provider(lease, &bank, &provider).await?;
		let policy = memory::policy(lease, &provider).await?;
		let roles = json!({"extraction":policy.extraction,"derivation":policy.derivation,"reflection":policy.reflection,"embedding":policy.embedding,"reranker":policy.reranker,"tokenizer":policy.tokenizer});
		let mut dependencies = vec![memory::definition(lease, &provider, "memory").await?];
		for (reference, kind) in [
			(&policy.extraction, "model"),
			(&policy.derivation, "model"),
			(&policy.reflection, "model"),
			(&policy.embedding, "embedding"),
			(&policy.reranker, "reranker"),
			(&policy.tokenizer, "tokenizer"),
		] {
			dependencies.push(memory::definition(lease, reference, kind).await?);
		}
		if let Some(access) = lease.access() {
			access.track_registry(&dependencies).await?;
		}
		let tokenizer: TokenizerConfig = serde_json::from_value(
			memory::definition(lease, &policy.tokenizer, "tokenizer")
				.await?
				.config,
		)?;
		match tokenizer {
			TokenizerConfig::Utf8UpperBound => {}
		}
		let placeholder =
			json!({"bank":bank,"provider":provider,"roles":roles,"recall":{"status":"no_space"}});
		let mut trial = envelope.clone();
		trial["banks"]
			.as_array_mut()
			.unwrap()
			.push(placeholder.clone());
		let overhead = serde_json::to_vec(&trial)?.len();
		if overhead > budget {
			if ordinal == 0 {
				return Ok(Context::status(bounded_status("no_space", budget)?));
			}
			break;
		}
		let available = budget - overhead;
		let max_tokens = available
			.min(declared_tokens)
			.min(policy.bounds.max_context_tokens);
		let recall = if max_tokens == 0 {
			Recall::NoSpace
		} else if repository::bank_id(lease, &bank, false).await?.is_none() {
			Recall::Empty
		} else {
			let limited_query = RecallQuery {
				text: bounded_query(&query, policy.bounds.max_input_bytes).to_owned(),
				time: None,
				kinds: vec![],
				max_tokens,
			};
			let key = serde_json::to_string(
				&json!({"purpose":"inference-memory","boundary":boundary,"provider":provider,"bank":bank,"query":limited_query}),
			)?;
			let result = memory::operate_staged(
				store,
				lease,
				Operation {
					operation_id: memory::request_id(run.id, &key)?,
					provider: provider.clone(),
					bank: bank.clone(),
					action: Action::Recall {
						query: limited_query,
					},
				},
				Some(run.id),
				&mut delivered,
			)
			.await?;
			let Outcome::Recall(recall) = result else {
				return Err(Error::SemanticUnavailable);
			};
			recall
		};
		let item = json!({"bank":bank,"provider":provider,"roles":roles,"recall":recall});
		envelope["banks"].as_array_mut().unwrap().push(item);
		if serde_json::to_vec(&envelope)?.len() > budget {
			return Err(Error::SemanticUnavailable);
		}
	}
	Ok(Context {
		value: Some(envelope),
		delivered,
	})
}

/// Journal once after every Bank and the combined delivery envelope have succeeded.
pub(crate) async fn complete(
	lease: &mut Lease<'_>,
	run: &Run,
	semantic: Option<Value>,
	memory: Context,
	budget: usize,
) -> Result<Option<Value>> {
	let output = combine(semantic, memory.value, budget)?;
	if output
		.as_ref()
		.is_some_and(|value| value["memory"]["banks"].is_array())
		&& !memory.delivered.is_empty()
	{
		super::super::repositories::memory_reads::record(lease, run.id, &memory.delivered).await?;
	}
	Ok(output)
}

/// Preserve existing Workspace context and include all wrapper bytes in the cap.
pub(crate) fn combine(
	semantic: Option<Value>,
	memory: Option<Value>,
	budget: usize,
) -> Result<Option<Value>> {
	let Some(memory) = memory else {
		return Ok(semantic);
	};
	let output = json!({"workspace":semantic,"memory":memory});
	if serde_json::to_vec(&output)?.len() > budget {
		return bounded_status("no_space", budget);
	}
	Ok(Some(output))
}

/// Truncate only the retrieval query; source identity and accepted-input revisions
/// remain complete in the envelope. UTF-8 characters are never split.
fn bounded_query(text: &str, limit: usize) -> &str {
	let mut end = text.len().min(limit);
	while !text.is_char_boundary(end) {
		end -= 1;
	}
	&text[..end]
}

fn bounded_status(status: &str, budget: usize) -> Result<Option<Value>> {
	let value = json!({"status": status});
	if serde_json::to_vec(&value)?.len() > budget {
		return Err(Error::RemoteSemantic(
			crate::semantic::remote::Failure::ContextBudget,
		));
	}
	Ok(Some(value))
}

/// Semantic index and memory participant revisions an Ordered Retrieval Key
/// pins. Plain reads: the recheck before every reuse enforces authority.
pub(crate) async fn source_revisions(
	store: &Store,
	run: &Run,
) -> Result<(Option<i64>, Option<i64>)> {
	use reinhardt::query::{
		Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	};
	let index: Option<i64> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("revision"))
			.from(Alias::new("semantic_indexes"))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::value(run.workspace_id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&store.pool)
	.await?;
	let participant: Option<i64> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("participant_revision"))
			.from(Alias::new("memory_run_bindings"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&store.pool)
	.await?;
	Ok((index, participant))
}

#[cfg(test)]
#[path = "memory_context/tests.rs"]
mod tests;

/// Recovered source observations retain current authority and canonical revisions.
pub(crate) async fn recheck(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &Run,
	observed: &Value,
) -> Result<()> {
	let workspace = observed.get("workspace").unwrap_or(observed);
	if !workspace.is_null() && workspace.get("status").is_none() {
		aidash_application::semantic::retrieval::recheck(
			&mut crate::bootstrap::semantic_retrieval_scope(store, lease),
			&serde_json::from_value(workspace.clone())?,
		)
		.await?;
	}
	if observed["memory"]["banks"].is_array()
		&& !super::super::repositories::memory_reads::visible(lease, run.id).await?
	{
		return Err(Error::Forbidden);
	}
	Ok(())
}
