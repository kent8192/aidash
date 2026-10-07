//! Native memory unit projections borrow the caller's authority transaction.
use super::access::Lease;
use crate::{Error, Result, database::native};
use aidash_domain::memory::{Bank, Content, Policy, TimeRange, Unit};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _,
};
use serde_json::json;
use uuid::Uuid;

pub(crate) async fn load(lease: &mut Lease<'_>, id: Uuid, exclusive: bool) -> Result<Option<Unit>> {
	lease.tx().pool().require_memory_serving()?;
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_units"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
			.lock(if exclusive {
				LockType::Update
			} else {
				LockType::Share
			})
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let Some(row) = row else {
		return Ok(None);
	};
	let bank_id: Uuid = row.try_get("bank_id")?;
	let bank = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_banks"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(bank_id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **lease.tx())
	.await?;
	let unit = Unit {
		id,
		bank: Bank {
			home: bank.try_get("home")?,
			tenant: bank.try_get("tenant")?,
			workspace: bank.try_get("workspace_id")?,
			participant: bank.try_get("participant_id")?,
		},
		revision: row.try_get("revision")?,
		content: content(&row)?,
		learned_at: row.try_get("learned_at")?,
		updated_at: row.try_get("updated_at")?,
		deleted: row.try_get("deleted")?,
		stale: row.try_get("stale")?,
	};
	lease.tx().require_memory_current(&unit)?;
	Ok(Some(unit))
}

pub(crate) fn content(row: &native::Row) -> Result<Content> {
	let start = row.try_get("occurred_start")?;
	let end = row.try_get("occurred_end")?;
	let occurred = match (start, end) {
		(Some(start), Some(end)) => Some(TimeRange { start, end }),
		(None, None) => None,
		_ => return Err(Error::Invalid("corrupt memory occurrence window".into())),
	};
	Ok(Content {
		mental_model: row.try_get("mental_model")?,
		text: row.try_get("text")?,
		kind: serde_json::from_value(json!(row.try_get::<String>("kind")?))?,
		learning: serde_json::from_value(json!(row.try_get::<String>("learning")?))?,
		verification: serde_json::from_value(json!(row.try_get::<String>("verification")?))?,
		occurred,
		entities: row.try_get("entities")?,
		evidence: row.try_get("evidence")?,
		links: row.try_get("links")?,
	})
}

pub(crate) async fn authorize(lease: &mut Lease<'_>, bank: &Bank, action: &str) -> Result<()> {
	lease.tx().pool().require_memory_serving()?;
	bank.validate()?;
	if let Some(access) = lease.access() {
		let workspace = access.workspace(bank.workspace).await?;
		if bank.home != access.node_id
			|| bank.tenant != workspace.tenant
			|| bank.tenant != access.identity.tenant
		{
			return Err(Error::Forbidden);
		}
		access.require(&workspace, "workspace.read").await?;
		let mut attributes = workspace.attributes;
		attributes["participant_id"] = json!(bank.participant);
		attributes["shared"] = json!(bank.participant.is_none());
		if let Some(id) = bank.participant {
			let row = native::query(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("memory_participants"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
			.ok_or(Error::Forbidden)?;
			if row.try_get::<bool>("deleted")?
				|| row.try_get::<String>("home")? != bank.home
				|| row.try_get::<String>("tenant")? != bank.tenant
				|| row.try_get::<Uuid>("workspace_id")? != bank.workspace
			{
				return Err(Error::Forbidden);
			}
			attributes["created_by"] = json!(row.try_get::<String>("principal")?);
			attributes["agent_id"] = json!(row.try_get::<String>("agent_id")?);
		}
		let resource = access.resource(
			"memory",
			bank.participant
				.map_or_else(|| bank.workspace.to_string(), |id| id.to_string()),
			attributes,
		);
		access.require(&resource, action).await?;
		Ok(())
	} else {
		// Operator access is still limited by current Home visibility; no executor bank union.
		crate::authorization::remote::operator::require(lease.tx(), bank.workspace).await?;
		if let Some(id) = bank.participant {
			let row = native::query(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("memory_participants"))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **lease.tx())
			.await?
			.ok_or(Error::Forbidden)?;
			if row.try_get::<bool>("deleted")?
				|| row.try_get::<String>("home")? != bank.home
				|| row.try_get::<String>("tenant")? != bank.tenant
				|| row.try_get::<Uuid>("workspace_id")? != bank.workspace
			{
				return Err(Error::Forbidden);
			}
		}
		Ok(())
	}
}

pub(crate) async fn text(
	lease: &mut Lease<'_>,
	id: Uuid,
	workspace: Uuid,
) -> Result<Option<String>> {
	let Some(unit) = load(lease, id, false).await? else {
		return Ok(None);
	};
	if unit.bank.workspace != workspace || !unit.visible() {
		return Ok(None);
	}
	authorize(lease, &unit.bank, "memory.read").await?;
	let policy = unexpired(lease, &unit).await?;
	current(
		lease,
		workspace,
		&unit.content.evidence,
		policy.bounds.max_graph_visits,
	)
	.await?;
	Ok(Some(unit.content.text))
}

/// TTL is a disclosure fence even before the bounded maintenance worker runs.
/// Keep raw loading available to deletion/restore paths so they can clean up.
pub(crate) async fn unexpired(lease: &mut Lease<'_>, unit: &Unit) -> Result<Policy> {
	let settings = super::bank_settings::get(lease, &unit.bank)
		.await?
		.ok_or(Error::Forbidden)?;
	let policy = crate::semantic::native_memory::policy(lease, &settings.provider).await?;
	if policy
		.retention
		.unit_expired(unit.learned_at, chrono::Utc::now())
	{
		return Err(Error::Conflict("memory unit retention expired".into()));
	}
	Ok(policy)
}

/// Traverse exact support revisions under the same policy/source locks. Cycles and
/// oversized provenance graphs fail closed rather than delivering partial evidence.
pub(crate) async fn current(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	evidence: &[aidash_domain::memory::Evidence],
	max_visits: usize,
) -> Result<()> {
	current_in(lease, workspace, evidence, max_visits, &mut 0).await
}

pub(crate) fn current_in<'a>(
	lease: &'a mut Lease<'_>,
	workspace: Uuid,
	evidence: &'a [aidash_domain::memory::Evidence],
	max_visits: usize,
	visits: &'a mut usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
	Box::pin(async move {
		use aidash_application::ports::semantic::visibility::SemanticDisclosureScope;
		use aidash_domain::memory::Evidence;
		let mut pending: std::collections::VecDeque<_> = evidence
			.iter()
			.cloned()
			.map(|e| (e, std::collections::BTreeSet::<(u8, Uuid)>::new()))
			.collect();
		let mut seen = std::collections::BTreeSet::new();
		while let Some((source, mut ancestry)) = pending.pop_front() {
			*visits = visits
				.checked_add(1)
				.ok_or_else(|| Error::Invalid("memory evidence traversal overflow".into()))?;
			if *visits > max_visits {
				return Err(Error::Invalid(
					"memory evidence traversal exceeds its bound".into(),
				));
			}
			if let Evidence::Unit { id, .. } = &source
				&& !ancestry.insert((0, *id))
			{
				return Err(Error::Invalid("cyclic memory support".into()));
			}
			if let Evidence::Run { id, .. } = &source
				&& !ancestry.insert((1, *id))
			{
				return Err(Error::Invalid("cyclic Run memory support".into()));
			}
			if !seen.insert(source.clone()) && !matches!(source, Evidence::Unit { .. }) {
				continue;
			}
			match source {
				Evidence::Publication { id, revision } => {
					Box::pin(super::publications::check(
						lease, workspace, id, revision, max_visits, visits,
					))
					.await?;
				}
				Evidence::Unit { bank, id, revision } => {
					if bank.workspace != workspace {
						return Err(Error::Forbidden);
					}
					let unit = load(lease, id, false)
						.await?
						.ok_or_else(|| Error::Conflict("memory support removed".into()))?;
					if unit.bank != bank || unit.revision != revision || !unit.visible() {
						return Err(Error::Conflict("memory support changed".into()));
					}
					authorize(lease, &bank, "memory.read").await?;
					unexpired(lease, &unit).await?;
					pending.extend(
						unit.content
							.evidence
							.into_iter()
							.map(|e| (e, ancestry.clone())),
					);
				}
				Evidence::Message {
					id,
					revision,
					digest,
				} => {
					let mut disclosure = super::disclosure::Disclosure { lease };
					let message = disclosure
						.message(id, workspace)
						.await?
						.ok_or_else(|| Error::Conflict("memory message source removed".into()))?;
					if disclosure.scoped() && !disclosure.message_visible(&message).await? {
						return Err(Error::Forbidden);
					}
					// Message bodies are immutable version 1; Run input sequence is tracked separately.
					if revision != 1
						|| aidash_domain::semantic::indexing::content_digest(&message.content)
							!= digest
					{
						return Err(Error::Conflict("memory message source changed".into()));
					}
				}
				Evidence::Artifact {
					id,
					revision,
					digest,
				} => {
					let mut disclosure = super::disclosure::Disclosure { lease };
					let artifact = disclosure
						.artifact(id, workspace)
						.await?
						.ok_or_else(|| Error::Conflict("memory artifact source removed".into()))?;
					if disclosure.scoped() && !disclosure.artifact_visible(&artifact).await? {
						return Err(Error::Forbidden);
					}
					// Current Artifact bodies are immutable version 1; no invented update version is accepted.
					if revision != 1
						|| aidash_domain::semantic::indexing::content_digest(
							&serde_json::to_string(&artifact.content)?,
						) != digest
					{
						return Err(Error::Conflict("memory artifact source changed".into()));
					}
				}
				Evidence::Run {
					id,
					revision,
					digest,
				} => {
					let row = native::query(
						&Query::select()
							.column(ColumnRef::Asterisk)
							.from(Alias::new("runs"))
							.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
							.lock(LockType::Share)
							.to_string(PostgresQueryBuilder),
					)
					.fetch_optional(&mut **lease.tx())
					.await?
					.ok_or_else(|| Error::Conflict("memory Run evidence removed".into()))?;
					if row.try_get::<Uuid>("workspace_id")? != workspace
						|| row.try_get::<i64>("revision")? != revision
						|| row.try_get::<String>("phase")? != "COMPLETED"
					{
						return Err(Error::Conflict(
							"memory Run evidence is incomplete or changed".into(),
						));
					}
					let run: aidash_domain::Run = crate::database::query_as(
						&Query::select()
							.column(ColumnRef::Asterisk)
							.from(Alias::new("runs"))
							.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
							.to_string(PostgresQueryBuilder),
					)
					.fetch_one(&mut **lease.tx())
					.await?;
					if run.error.is_some() {
						return Err(Error::Conflict(
							"memory Run evidence has a failed outcome".into(),
						));
					}
					// A Run can carry an admitted unit into extracted candidates or
					// later memory. Preserve this exact dependency for operator
					// maintenance as well as subject disclosure, within the same
					// traversal budget and ancestry (no recursive budget reset).
					let reads = native::query(
						&Query::select()
							.columns(["unit_id", "revision"].map(Alias::new))
							.from(Alias::new("memory_run_reads"))
							.and_where(Expr::col("run_id").eq(Expr::value(id)))
							.limit(max_visits as u64 + 1)
							.lock(LockType::Share)
							.to_string(PostgresQueryBuilder),
					)
					.fetch_all(&mut **lease.tx())
					.await?;
					if reads.len() > max_visits {
						return Err(Error::Invalid(
							"Run memory support exceeds its bound".into(),
						));
					}
					if !reads.is_empty() {
						let binding = super::bindings::load(&mut **lease.tx(), &run.metadata())
							.await?
							.ok_or(Error::Forbidden)?;
						authorize(lease, &binding.bank, "memory.read").await?;
					}
					for read in reads {
						let unit = load(lease, read.try_get("unit_id")?, false)
							.await?
							.ok_or(Error::Forbidden)?;
						pending.push_back((
							Evidence::Unit {
								bank: unit.bank,
								id: unit.id,
								revision: read.try_get("revision")?,
							},
							ancestry.clone(),
						));
					}
					let effects = native::query(
						&Query::select()
							.column(Alias::new("status"))
							.from(Alias::new("invocations"))
							.and_where(Expr::col("run_id").eq(Expr::value(id)))
							.limit(max_visits as u64 + 1)
							.lock(LockType::Share)
							.to_string(PostgresQueryBuilder),
					)
					.fetch_all(&mut **lease.tx())
					.await?;
					if effects.len() > max_visits
						|| effects.iter().any(|effect| {
							effect
								.try_get::<String>("status")
								.map_or(true, |state| state != "COMPLETED")
						}) {
						return Err(Error::Conflict(
							"memory Run evidence contains incomplete or uncertain effects".into(),
						));
					}
					if let Some(access) = lease.access()
						&& !access.run_visible(&run).await?
					{
						return Err(Error::Forbidden);
					}
					if aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(
						&run,
					)?) != digest
					{
						return Err(Error::Conflict("memory Run evidence digest changed".into()));
					}
				}
			}
		}
		Ok(())
	})
}
