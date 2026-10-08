//! Home-issued native memory mappings retain current participant, role and disclosure authority.
use super::{memory_models::Models, native_memory as memory};
use crate::apps::knowledge::repositories::{
	access::Lease, memory_scope::Scope, native_memory as repository,
};
use crate::{Error, Result, database::native, federation::Federation, store::Store};
use aidash_domain::{
	memory::*,
	registry::{EntityRef, Entry},
	semantic::remote::{
		NativeBank, NativeBinding, NativeContext, NativeOrigin, NativeRecall, NativeRequest,
		Operation, Provider,
	},
};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::json;
use uuid::Uuid;

pub(crate) fn descriptor(home: &str, entry: &Entry) -> Result<Provider> {
	Ok(Provider {
		node_id: home.into(),
		entry: EntityRef {
			id: entry.id.clone(),
			version: entry.version.clone(),
		},
		digest: aidash_domain::registry::rules::digest(&serde_json::to_value(entry)?),
		configuration_digest: aidash_domain::registry::rules::digest(&entry.config),
	})
}

pub(crate) async fn bind(
	home: &str,
	lease: &mut Lease<'_>,
	workspace: Uuid,
	selection: &NativeRequest,
	generation: Option<&NativeOrigin>,
) -> Result<NativeBinding> {
	if selection.participant.is_nil() || selection.expected_revision < 1 {
		return Err(Error::Invalid(
			"native Home memory requires an observed logical participant revision".into(),
		));
	}
	let tenant: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("tenant"))
			.from(Alias::new("authorization_workspaces"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await?;
	let mut bank = Bank {
		home: home.into(),
		tenant,
		workspace,
		participant: Some(selection.participant),
	};
	for action in ["memory.participant.use"] {
		if bank.home != home {
			return Err(Error::Forbidden);
		}
		super::super::repositories::units::authorize(lease, &bank, action).await?;
	}
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_participants"))
			.and_where(Expr::col("id").eq(Expr::value(selection.participant)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **lease.tx())
	.await?;
	if row.try_get::<i64>("revision")? != selection.expected_revision {
		return Err(Error::Conflict("Home logical participant changed".into()));
	}
	let reference = EntityRef {
		id: row.try_get("agent_id")?,
		version: row.try_get("agent_version")?,
	};
	let agent = memory::definition(lease, &reference, "agent").await?;
	let config = super::super::repositories::bindings::configuration(lease, &agent, home).await?;
	if config.memory.as_ref() != Some(&selection.provider)
		|| config.allow_cross_conversation_memory == Some(false)
	{
		return Err(Error::Conflict(
			"selected Home logical Agent does not enable this memory provider".into(),
		));
	}
	if let Some(origin) = generation {
		aidash_domain::configuration::validate_node_id(&origin.node_id)?;
		if origin.node_id == home || origin.intent_id.is_nil() {
			return Err(Error::Forbidden);
		}
		bank = super::super::repositories::remote_participants::issue(
			lease,
			&bank,
			&reference,
			&selection.provider,
			origin,
		)
		.await?;
	}
	let mut declared = vec![(bank.clone(), selection.provider.clone(), usize::MAX)];
	for source in &config.sources {
		let entry = memory::definition(lease, source, "source").await?;
		let source: SourceConfig = serde_json::from_value(entry.config)?;
		if source.max_tokens == 0 {
			return Err(Error::Invalid(
				"Home memory Source requires a finite context limit".into(),
			));
		}
		let mut selected = bank.clone();
		if source.scope == SourceScope::Workspace {
			selected.participant = None;
		}
		super::memory_context::declare_bank(
			&mut declared,
			selected,
			source.memory,
			source.max_tokens,
		);
	}
	let mut banks = Vec::new();
	for (bank, provider, tokens) in declared {
		for action in ["memory.read", "memory.disclose"] {
			if bank.home != home {
				return Err(Error::Forbidden);
			}
			super::super::repositories::units::authorize(lease, &bank, action).await?;
		}
		memory::bank_provider(lease, &bank, &provider).await?;
		let policy = memory::policy(lease, &provider).await?;
		let definition = memory::definition(lease, &provider, "memory").await?;
		let mut roles = Vec::new();
		for (reference, kind) in [
			(&policy.extraction, "model"),
			(&policy.derivation, "model"),
			(&policy.reflection, "model"),
			(&policy.embedding, "embedding"),
			(&policy.reranker, "reranker"),
			(&policy.tokenizer, "tokenizer"),
		] {
			let entry = memory::definition(lease, reference, kind).await?;
			if kind == "reranker"
				&& let RerankerConfig::Model { model } =
					serde_json::from_value(entry.config.clone())?
			{
				roles.push(descriptor(
					home,
					&memory::definition(lease, &model, "model").await?,
				)?);
			}
			let descriptor = descriptor(home, &entry)?;
			if !roles.contains(&descriptor) {
				roles.push(descriptor);
			}
		}
		banks.push(NativeBank {
			bank,
			provider: descriptor(home, &definition)?,
			roles,
			max_model_tokens: policy.bounds.max_model_tokens,
			max_context_tokens: tokens.min(policy.bounds.max_context_tokens),
			cache_max_age_seconds: u64::from(policy.retention.model_result_days) * 86_400,
			cache_max_attempts: policy.bounds.max_retries,
		});
	}
	let revision: i64 = native::query_scalar(
		&Query::select()
			.column(Alias::new("revision"))
			.from(Alias::new("memory_participants"))
			.and_where(Expr::col("id").eq(Expr::value(bank.participant.ok_or(Error::Forbidden)?)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await?;
	Ok(NativeBinding {
		selection: selection.clone(),
		generation: generation.cloned(),
		participant: Binding {
			bank,
			participant_revision: revision,
			agent: reference,
			provider: selection.provider.clone(),
		},
		agent: descriptor(home, &agent)?,
		banks,
	})
}

pub(crate) async fn stamp(
	store: &Store,
	lease: &mut Lease<'_>,
	binding: &NativeBinding,
) -> Result<String> {
	let current = bind(
		&store.node_id,
		lease,
		binding.participant.bank.workspace,
		&binding.selection,
		binding.generation.as_ref(),
	)
	.await?;
	if &current != binding {
		return Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::Configuration,
		));
	}
	let mut revisions = vec![];
	for declared in &binding.banks {
		let revision = if let Some(id) = repository::bank_id(lease, &declared.bank, false).await? {
			Some(
				native::query_scalar::<i64>(
					&Query::select()
						.column(Alias::new("revision"))
						.from(Alias::new("memory_banks"))
						.and_where(Expr::col("id").eq(Expr::value(id)))
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **lease.tx())
				.await?,
			)
		} else {
			None
		};
		revisions.push((&declared.bank, revision));
	}
	Ok(aidash_domain::registry::rules::digest(
		&json!({"binding":binding,"revisions":revisions,"authority":lease.saved()?}),
	))
}

pub(crate) async fn retrieve(
	runtime: &Federation,
	lease: &mut Lease<'_>,
	receiver: &str,
	binding: &NativeBinding,
	operation: &Operation,
	budget: usize,
) -> Result<NativeContext> {
	stamp(&runtime.store, lease, binding).await?;
	let mut context = NativeContext { banks: vec![] };
	for declared in &binding.banks {
		let policy = memory::policy(lease, &declared.provider.entry).await?;
		let placeholder = NativeRecall {
			bank: declared.bank.clone(),
			provider: declared.provider.entry.clone(),
			recall: Recall::NoSpace,
		};
		let mut trial = context.clone();
		trial.banks.push(placeholder.clone());
		let overhead = serde_json::to_vec(&trial)?.len();
		if overhead > budget {
			return Err(Error::RemoteSemantic(
				aidash_domain::semantic::Failure::ContextBudget,
			));
		}
		let available = (budget - overhead).min(declared.max_context_tokens);
		let recall = if available == 0 {
			Recall::NoSpace
		} else if repository::bank_id(lease, &declared.bank, false)
			.await?
			.is_none()
		{
			Recall::Empty
		} else {
			let query = RecallQuery {
				text: aidash_domain::semantic::remote::bounded_query(
					&operation.query,
					policy.bounds.max_input_bytes,
				)
				.0,
				time: None,
				kinds: vec![],
				max_tokens: available,
			};
			let digest = aidash_domain::registry::rules::digest(
				&json!({"operation":operation,"bank":declared.bank,"provider":declared.provider,"query":query}),
			);
			let id = memory::request_id(operation.id, &digest)?;
			let mut models = Models::resolve(
				&runtime.store,
				lease,
				declared.provider.entry.clone(),
				declared.bank.clone(),
				policy.clone(),
				id,
				digest,
				None,
			)
			.await?;
			models.remote = Some(super::remote_memory_models::Origin {
				runtime: runtime.clone(),
				receiver: receiver.into(),
				operation: operation.clone(),
				binding: binding.clone(),
			});
			let engine = aidash_application::memory::Engine {
				provider: &declared.provider.entry,
				policy: &policy,
				models: &models,
			};
			let mut scope = Scope {
				store: &runtime.store,
				lease,
				models: &models,
				delivered: &mut Vec::new(),
			};
			engine.recall(&mut scope, &declared.bank, &query).await?
		};
		context.banks.push(NativeRecall {
			recall,
			..placeholder
		});
		if serde_json::to_vec(&context)?.len() > budget {
			return Err(Error::RemoteSemantic(
				aidash_domain::semantic::Failure::ContextBudget,
			));
		}
	}
	Ok(context)
}
