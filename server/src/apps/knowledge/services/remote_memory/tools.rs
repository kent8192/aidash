//! Memory tools of a Run that a Remote Grant started, served by its Home (ADR 0126, 0139).
//!
//! The banks come from the Home's saved binding of the grant, revalidated against the Home's
//! current state, and never from the request. A recall reads the published Workspace bank and
//! the Agent's participant bank; a write goes only to the participant bank. The operation id
//! of a call is derived from the admission and the call's key, so a resumed Run asks the same
//! operation again (ADR 0057).
use super::super::native_memory as memory;
use crate::apps::knowledge::repositories::{
	access::Lease, native_memory as repository, remote_memory_reads, units,
};
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{
	memory::{Change, Evidence, Recall, RecallQuery, Unit, Verification},
	semantic::{
		Failure,
		remote::{Binding as Semantic, NativeBinding, NativeContext, NativeRecall},
	},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

/// The most Units one tool recall returns, as for a local Run's shared recall.
const MAX_UNITS: usize = 8;

/// The grant's native binding, still current on the Home.
async fn binding(store: &Store, lease: &mut Lease<'_>, grant: Uuid) -> Result<NativeBinding> {
	let saved: Option<serde_json::Value> = native::query_scalar(
		&Query::select()
			.column(Alias::new("semantic"))
			.from(Alias::new("authorization_remote_grants"))
			.and_where(Expr::col("id").eq(Expr::value(grant)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **lease.tx())
	.await?;
	let semantic: Semantic = serde_json::from_value(saved.ok_or(Error::Forbidden)?)?;
	let binding = semantic
		.native()
		.ok_or_else(|| Error::Invalid("this Remote Grant carries no Home memory".into()))?
		.clone();
	let current = super::bind(
		store.deployment.as_str(),
		lease,
		binding.participant.bank.workspace,
		&binding.selection,
		binding.generation.as_ref(),
	)
	.await?;
	if current != binding {
		return Err(Error::RemoteSemantic(Failure::Invalidated));
	}
	Ok(binding)
}

fn operation_id(admission: Uuid, key: &str, bank: &aidash_domain::memory::Bank) -> Result<Uuid> {
	memory::request_id(
		admission,
		&format!(
			"remote-tool:{key}:{}:{:?}",
			bank.workspace, bank.participant
		),
	)
}

/// A recall or a reflection. A recall merges what both banks return, within the call's token
/// budget, and records each returned Unit against the grant so a later change to it withdraws
/// what the Run consumed, as for the Run's starting context.
pub(crate) async fn recall(
	store: &Store,
	lease: &mut Lease<'_>,
	grant: Uuid,
	admission: Uuid,
	key: &str,
	query: RecallQuery,
	reflect: bool,
) -> Result<memory::Outcome> {
	let binding = binding(store, lease, grant).await?;
	if reflect {
		// A reflection is the Agent's own synthesis over its participant bank.
		let declared = binding
			.read_banks()
			.iter()
			.find(|declared| declared.bank == *binding.write_bank())
			.ok_or(Error::Forbidden)?;
		units::authorize(lease, &declared.bank, "memory.disclose").await?;
		return memory::operate_in(
			store,
			lease,
			memory::Operation {
				operation_id: operation_id(admission, key, &declared.bank)?,
				provider: declared.provider.entry.clone(),
				bank: declared.bank.clone(),
				action: memory::Action::Reflect { query },
			},
			None,
		)
		.await;
	}
	let mut selected: Vec<Unit> = Vec::new();
	let mut context = NativeContext { banks: vec![] };
	for declared in binding.read_banks() {
		let own = declared.bank == *binding.write_bank();
		let read = async {
			units::authorize(lease, &declared.bank, "memory.read").await?;
			units::authorize(lease, &declared.bank, "memory.disclose").await?;
			memory::operate_in(
				store,
				lease,
				memory::Operation {
					operation_id: operation_id(admission, key, &declared.bank)?,
					provider: declared.provider.entry.clone(),
					bank: declared.bank.clone(),
					action: memory::Action::Recall {
						query: query.clone(),
					},
				},
				None,
			)
			.await
		}
		.await;
		let outcome = match read {
			Ok(outcome) => outcome,
			// A published bank this chain may not read is left out; the Agent's own bank is not.
			Err(Error::Forbidden | Error::Conflict(_)) if !own => continue,
			Err(error) => return Err(error),
		};
		let mut kept = Vec::new();
		if let memory::Outcome::Recall(Recall::Ready { units }) = outcome {
			for unit in units {
				if selected.len() >= MAX_UNITS {
					break;
				}
				let mut candidate = selected.clone();
				candidate.push(unit.clone());
				if serde_json::to_vec(&memory::Outcome::Recall(Recall::Ready { units: candidate }))?
					.len() <= query.max_tokens
				{
					selected.push(unit.clone());
					kept.push(unit);
				}
			}
		}
		context.banks.push(NativeRecall {
			bank: declared.bank.clone(),
			provider: declared.provider.entry.clone(),
			recall: if kept.is_empty() {
				Recall::Empty
			} else {
				Recall::Ready { units: kept }
			},
		});
	}
	if !selected.is_empty() {
		let capacity = remote_memory_reads::capacity(lease, &binding).await?;
		remote_memory_reads::record(lease.tx(), grant, &binding, &context, capacity).await?;
	}
	Ok(memory::Outcome::Recall(if selected.is_empty() {
		Recall::Empty
	} else {
		Recall::Ready { units: selected }
	}))
}

/// A write, to the Agent's participant bank only. The Home holds no Journal of a Run that
/// executes elsewhere, so a change whose evidence is that Journal is refused with its reason
/// instead of failing inside the write; built-in memory facts need exactly that evidence.
pub(crate) async fn mutate(
	store: &Store,
	lease: &mut Lease<'_>,
	grant: Uuid,
	admission: Uuid,
	key: &str,
	changes: &[Change],
) -> Result<Vec<Unit>> {
	let binding = binding(store, lease, grant).await?;
	if !binding.writes_private_bank() {
		return Err(Error::Forbidden);
	}
	let mutation = aidash_domain::memory::Mutation {
		operation_id: memory::request_id(admission, key)?,
		provider: binding.participant.provider.clone(),
		bank: binding.write_bank().clone(),
		changes: changes.to_vec(),
	};
	repository::lock_workspace(lease, mutation.bank.workspace, true).await?;
	memory::scope(store, lease, &mutation.bank, "memory.write").await?;
	memory::bank_provider(lease, &mutation.bank, &mutation.provider).await?;
	let policy = memory::policy(lease, &mutation.bank.tenant, &mutation.provider).await?;
	let builtin = mutation.provider.id
		== aidash_domain::registry::bindings::memory_defaults::MEMORY_ID
		&& mutation.provider.version == aidash_domain::registry::bindings::memory_defaults::VERSION;
	let model_status = if builtin {
		Verification::Recorded
	} else {
		Verification::Unverified
	};
	for change in &mutation.changes {
		if let Change::Add { content, .. } | Change::Correct { content, .. } = change {
			if content.verification != model_status {
				return Err(Error::Forbidden);
			}
			if builtin
				|| content.evidence.iter().any(
					|source| matches!(source, Evidence::JournalRange { run_id, .. } if *run_id == admission),
				) {
				return Err(Error::Invalid(
					"this Home does not hold the Journal of a Run that executes on another Node, so it cannot verify Journal evidence for a memory write".into(),
				));
			}
		}
		if let Change::Correct { id, .. } | Change::Delete { id, .. } = change {
			let existing = units::load(lease, *id, false)
				.await?
				.ok_or(Error::Forbidden)?;
			if existing.bank != mutation.bank || existing.content.verification != model_status {
				return Err(Error::Forbidden);
			}
		}
	}
	memory::validate_direct_mutation(lease, &mutation, &policy).await?;
	repository::mutate(lease, &mutation, &policy.bounds).await
}
