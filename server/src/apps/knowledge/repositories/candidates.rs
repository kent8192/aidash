//! Candidates remain separate from admitted memory; human review shares unit CAS atomicity.
use super::{access::Lease, native_memory as repository, units};
use crate::{Error, Result, database::native};
use aidash_domain::memory::*;
use chrono::Utc;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;

pub(crate) const QUEUE_FULL: &str = "candidate review queue is full";

pub(crate) fn human(lease: &mut Lease<'_>) -> Result<()> {
	if let Some(access) = lease.access() {
		let subject = access
			.snapshot
			.bundle
			.subjects
			.get(&access.identity.subject)
			.ok_or(Error::Forbidden)?;
		if subject.kind != aidash_domain::policy::SubjectKind::User
			|| access.read_run.is_some()
			|| access.read_grant.is_some()
			|| access.subjects.iter().any(|id| {
				access
					.snapshot
					.bundle
					.subjects
					.get(id)
					.is_some_and(|subject| {
						subject.kind == aidash_domain::policy::SubjectKind::Agent
					})
			}) {
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}
fn candidate(row: &native::Row, bank: &Bank) -> Result<Candidate> {
	Ok(Candidate {
		id: row.try_get("id")?,
		bank: bank.clone(),
		revision: row.try_get("revision")?,
		run: row.try_get("run")?,
		content: units::content(row)?,
		state: serde_json::from_value(serde_json::json!(row.try_get::<String>("state")?))?,
	})
}
async fn unexpired(lease: &mut Lease<'_>, bank: &Bank, row: &native::Row) -> Result<()> {
	let settings = super::bank_settings::get(lease, bank)
		.await?
		.ok_or(Error::Forbidden)?;
	let policy = crate::semantic::native_memory::policy(lease, &settings.provider).await?;
	let created: chrono::DateTime<Utc> = row.try_get("created_at")?;
	if created
		.checked_add_signed(chrono::Duration::days(i64::from(
			policy.retention.candidate_days,
		)))
		.is_none_or(|expires| expires <= Utc::now())
	{
		return Err(Error::Conflict("memory candidate retention expired".into()));
	}
	Ok(())
}
pub(crate) async fn list(
	lease: &mut Lease<'_>,
	bank: &Bank,
	bounds: &Bounds,
) -> Result<Vec<Candidate>> {
	units::authorize(lease, bank, "memory.candidate.read").await?;
	let Some(id) = repository::bank_id(lease, bank, false).await? else {
		return Ok(vec![]);
	};
	let rows = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_candidates"))
			.and_where(Expr::col("bank_id").eq(Expr::value(id)))
			.and_where(Expr::col("state").eq("pending"))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(bounds.max_candidates as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	if rows.len() > bounds.max_candidates {
		return Err(Error::Conflict(
			"candidate review queue exceeds its bound".into(),
		));
	}
	let mut result = vec![];
	for row in rows {
		let value = candidate(&row, bank)?;
		let current = async {
			unexpired(lease, bank, &row).await?;
			units::current(
				lease,
				bank.workspace,
				&value.content.evidence,
				bounds.max_graph_visits,
			)
			.await
		}
		.await;
		match current {
			Ok(()) => result.push(value),
			Err(Error::Conflict(_) | Error::Forbidden) => {
				let mut hidden = value;
				hidden.state = CandidateState::Invalidated;
				hidden.content.text.clear();
				hidden.content.mental_model = None;
				hidden.content.occurred = None;
				hidden.content.entities.clear();
				hidden.content.evidence.clear();
				hidden.content.links.clear();
				result.push(hidden);
			}
			Err(error) => return Err(error),
		}
	}
	Ok(result)
}
pub(crate) async fn propose(
	lease: &mut Lease<'_>,
	bank: &Bank,
	run: &Evidence,
	content: &[Content],
	bounds: &Bounds,
) -> Result<Vec<Candidate>> {
	repository::lock_workspace(lease, bank.workspace, true).await?;
	units::authorize(lease, bank, "memory.candidate.propose").await?;
	let Evidence::Run { id: run_id, .. } = run else {
		return Err(Error::Invalid("learning needs a complete Run proof".into()));
	};
	let participant = bank.participant.ok_or(Error::Forbidden)?;
	let binding = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_run_bindings"))
			.and_where(Expr::col("run_id").eq(Expr::value(*run_id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	.ok_or(Error::Forbidden)?;
	if binding.try_get::<Uuid>("participant_id")? != participant {
		return Err(Error::Forbidden);
	}
	let provider = aidash_domain::registry::EntityRef {
		id: binding.try_get("provider_id")?,
		version: binding.try_get("provider_version")?,
	};
	let policy = crate::semantic::native_memory::policy(lease, &provider).await?;
	if !policy.learn_from_runs {
		return Err(Error::Forbidden);
	}
	units::current(
		lease,
		bank.workspace,
		std::slice::from_ref(run),
		bounds.max_graph_visits,
	)
	.await?;
	let bank_id = repository::bank_id(lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	let mut result = vec![];
	for item in content {
		item.validate(bounds)?;
		if item.kind.derived()
			|| item.verification != Verification::Unverified
			|| !item.evidence.contains(run)
		{
			return Err(Error::Invalid(
				"candidate must retain unverified complete Run provenance".into(),
			));
		}
		units::current(
			lease,
			bank.workspace,
			&item.evidence,
			bounds.max_graph_visits,
		)
		.await?;
		let digest = aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(&(
			run_id, item,
		))?);
		let mut bytes = [0u8; 16];
		for (i, b) in bytes.iter_mut().enumerate() {
			*b = u8::from_str_radix(&digest[i * 2..i * 2 + 2], 16)
				.map_err(|_| Error::Invalid("candidate identity digest".into()))?;
		}
		let id = Uuid::from_bytes(bytes);
		if let Some(row) = native::query(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("memory_candidates"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?
		{
			if row.try_get::<Uuid>("bank_id")? != bank_id {
				return Err(Error::Conflict("candidate identity collision".into()));
			}
			unexpired(lease, bank, &row).await?;
			result.push(candidate(&row, bank)?);
			continue;
		}
		repository::record_capacity(
			lease,
			bank_id,
			"memory_candidates",
			policy.retention.max_model_operations,
		)
		.await?;
		let count: i64 = native::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(Expr::col("id").into()))
				.from(Alias::new("memory_candidates"))
				.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
				.and_where(Expr::col("state").eq("pending"))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **lease.tx())
		.await?;
		if count >= bounds.max_candidates as i64 {
			return Err(Error::Conflict(QUEUE_FULL.into()));
		}
		let now = Utc::now();
		let mut columns = ["id", "bank_id", "revision", "run"]
			.map(Alias::new)
			.to_vec();
		columns.extend(repository::content_columns());
		columns.extend(["state", "created_at", "updated_at"].map(Alias::new));
		let mut select = Query::select();
		select
			.expr(Expr::value(id))
			.expr(Expr::value(bank_id))
			.expr(Expr::value(1_i64))
			.expr(Expr::value(serde_json::to_value(run)?));
		for value in repository::content_values(item)? {
			select.expr(value);
		}
		select
			.expr(Expr::value("pending"))
			.expr(Expr::value(now))
			.expr(Expr::value(now));
		native::query(
			&Query::insert()
				.into_table(Alias::new("memory_candidates"))
				.columns(columns)
				.from_subquery(select)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		result.push(Candidate {
			id,
			bank: bank.clone(),
			revision: 1,
			run: run.clone(),
			content: item.clone(),
			state: CandidateState::Pending,
		});
	}
	Ok(result)
}
pub(crate) async fn review(
	lease: &mut Lease<'_>,
	bank: &Bank,
	operation: Uuid,
	id: Uuid,
	expected: i64,
	mutation: Option<&Mutation>,
	bounds: &Bounds,
) -> Result<Option<Unit>> {
	human(lease)?;
	repository::lock_workspace(lease, bank.workspace, true).await?;
	units::authorize(lease, bank, "memory.candidate.review").await?;
	let bank_id = repository::bank_id(lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	if operation.is_nil() {
		return Err(Error::Invalid("review operation ID is required".into()));
	}
	let digest = aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(&(
		bank, id, expected, mutation,
	))?);
	if let Some(receipt) = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_candidate_reviews"))
			.and_where(Expr::col("operation_id").eq(Expr::value(operation)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	{
		if receipt.try_get::<Uuid>("bank_id")? != bank_id
			|| receipt.try_get::<String>("digest")? != digest
		{
			return Err(Error::Conflict(
				"review key reused for a different disposition".into(),
			));
		}
		// This non-null JSONB column stores JSON null for a rejected candidate.
		// Native optional projections distinguish SQL NULL from a JSON value, so
		// decode the JSON document before interpreting the optional proof.
		let proof: Option<Evidence> =
			serde_json::from_value(receipt.try_get::<serde_json::Value>("outcome")?)?;
		if let Some(Evidence::Unit { id, revision, .. }) = proof {
			let unit = units::load(lease, id, false)
				.await?
				.ok_or(Error::Forbidden)?;
			if unit.bank != *bank || unit.revision != revision || !unit.visible() {
				return Err(Error::Conflict("reviewed unit changed".into()));
			}
			units::unexpired(lease, &unit).await?;
			units::current(
				lease,
				bank.workspace,
				&unit.content.evidence,
				bounds.max_graph_visits,
			)
			.await?;
			return Ok(Some(unit));
		}
		return Ok(None);
	}
	let settings = super::bank_settings::get(lease, bank)
		.await?
		.ok_or(Error::Forbidden)?;
	let policy = crate::semantic::native_memory::policy(lease, &settings.provider).await?;
	repository::record_capacity(
		lease,
		bank_id,
		"memory_candidate_reviews",
		policy.retention.max_model_operations,
	)
	.await?;
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_candidates"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	.ok_or(Error::Forbidden)?;
	if row.try_get::<Uuid>("bank_id")? != bank_id {
		return Err(Error::Forbidden);
	}
	let item = candidate(&row, bank)?;
	if expected < 1
		|| item.revision != expected
		|| !matches!(
			item.state,
			CandidateState::Pending | CandidateState::Invalidated
		) {
		return Err(Error::Conflict(
			"candidate revision or disposition changed".into(),
		));
	}
	let mut admitted = None;
	if let Some(mutation) = mutation {
		unexpired(lease, bank, &row).await?;
		if item.state != CandidateState::Pending {
			return Err(Error::Conflict(
				"expired candidate cannot be admitted".into(),
			));
		}
		if mutation.bank != *bank || mutation.changes.len() != 1 {
			return Err(Error::Invalid(
				"review admits one unit in the candidate bank".into(),
			));
		}
		if let Change::Correct { id, .. } = &mutation.changes[0] {
			let target = units::load(lease, *id, false)
				.await?
				.ok_or(Error::Forbidden)?;
			if target.bank != *bank {
				return Err(Error::Forbidden);
			}
			if target.content.kind.derived() {
				return Err(Error::Invalid(
					"candidate review cannot change a derived memory kind; use the derive operation".into(),
				));
			}
		}
		let content = match &mutation.changes[0] {
			Change::Add { content, .. } | Change::Correct { content, .. } => content,
			Change::Delete { .. } => {
				return Err(Error::Invalid("review cannot delete a unit".into()));
			}
		};
		units::current(
			lease,
			bank.workspace,
			&item.content.evidence,
			bounds.max_graph_visits,
		)
		.await?;
		if content.kind != item.content.kind
			|| content.kind.derived()
			|| item
				.content
				.evidence
				.iter()
				.any(|source| !content.evidence.contains(source))
		{
			return Err(Error::Invalid(
				"review must preserve all candidate evidence and its world or experience kind"
					.into(),
			));
		}
		let Evidence::Run { id: origin, .. } = item.run else {
			return Err(Error::Forbidden);
		};
		admitted = repository::mutate_origin(lease, mutation, bounds, Some(origin))
			.await?
			.into_iter()
			.next();
	}
	native::query(
		&Query::update()
			.table(Alias::new("memory_candidates"))
			.value_expr(
				Alias::new("revision"),
				Expr::value(
					expected
						.checked_add(1)
						.ok_or_else(|| Error::Invalid("candidate revision overflow".into()))?,
				),
			)
			.value_expr(
				Alias::new("state"),
				Expr::value(if admitted.is_some() {
					"admitted"
				} else {
					"rejected"
				}),
			)
			.value_expr(Alias::new("updated_at"), Expr::value(Utc::now()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	let actor = lease.saved()?["subject"]
		.as_str()
		.ok_or(Error::Forbidden)?
		.to_owned();
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_candidate_reviews"))
			.columns(
				[
					"operation_id",
					"candidate_id",
					"bank_id",
					"observed_revision",
					"digest",
					"actor",
					"outcome",
					"created_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(operation))
					.expr(Expr::value(id))
					.expr(Expr::value(bank_id))
					.expr(Expr::value(expected))
					.expr(Expr::value(digest))
					.expr(Expr::value(actor))
					.expr(Expr::value(serde_json::to_value(
						admitted.as_ref().map(Unit::evidence),
					)?))
					.expr(Expr::value(Utc::now()))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(admitted)
}
