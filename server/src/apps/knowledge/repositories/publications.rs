//! Selected disclosure is an explicit grant; it does not confer private-bank authority.
use super::{access::Lease, candidates::human, native_memory as repository, units};
use crate::{Error, Result, database::native};
use aidash_domain::memory::*;
use chrono::Utc;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

/// Resolve negative lineage without admitting a withdrawn disclosure. The
/// immutable source identity remains necessary for transitive body purge.
pub(crate) async fn retained_source(lease: &mut Lease<'_>, id: Uuid) -> Result<(Uuid, i64)> {
	let row = native::query(
		&Query::select()
			.columns(["source_id", "source_revision"].map(Alias::new))
			.from(Alias::new("memory_publications"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	.ok_or(Error::Forbidden)?;
	Ok((row.try_get("source_id")?, row.try_get("source_revision")?))
}

pub(crate) async fn source(lease: &mut Lease<'_>, id: Uuid, revision: i64) -> Result<(Uuid, i64)> {
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_publications"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	.ok_or(Error::Forbidden)?;
	if row.try_get::<i64>("revision")? != revision || row.try_get::<bool>("deleted")? {
		return Err(Error::Conflict("memory disclosure grant changed".into()));
	}
	Ok((row.try_get("source_id")?, row.try_get("source_revision")?))
}
pub(crate) async fn check(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	id: Uuid,
	revision: i64,
	max_visits: usize,
	visits: &mut usize,
) -> Result<()> {
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_publications"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	.ok_or(Error::Forbidden)?;
	if row.try_get::<i64>("revision")? != revision || row.try_get::<bool>("deleted")? {
		return Err(Error::Conflict("memory disclosure grant changed".into()));
	}
	let bank_id: Uuid = row.try_get("bank_id")?;
	let bank = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_banks"))
			.and_where(Expr::col("id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **lease.tx())
	.await?;
	let destination = Bank {
		home: bank.try_get("home")?,
		tenant: bank.try_get("tenant")?,
		workspace: bank.try_get("workspace_id")?,
		participant: bank.try_get("participant_id")?,
	};
	if destination.workspace != workspace || destination.participant.is_some() {
		return Err(Error::Forbidden);
	}
	units::authorize(lease, &destination, "memory.read").await?;
	let source_id: Uuid = row.try_get("source_id")?;
	let source_revision: i64 = row.try_get("source_revision")?;
	let saved: crate::apps::knowledge::serializers::service::SavedAuthority =
		serde_json::from_value(row.try_get("authority")?)?;
	if let Some(credential_id) = saved.credential {
		let identity = crate::authorization::identity::SubjectIdentity {
			http_session: None,
			credential_id,
			tenant: saved.tenant,
			subject: saved.subject,
		};
		if identity.tenant != destination.tenant || !saved.subjects.contains(&identity.subject) {
			return Err(Error::Forbidden);
		}
		if let Some(access) = lease.access() {
			let mut publisher = access.publisher_view(identity, saved.subjects).await?;
			let result = Box::pin(check_source(
				&mut Lease::Inherited(&mut publisher),
				&destination,
				source_id,
				source_revision,
				max_visits,
				visits,
			))
			.await;
			return result;
		}
		// Operator views use the same physical transaction too: a queued source
		// writer must never sit between the reader and a second publisher lease.
		let snapshot = identity.lock_with_mode(lease.tx(), false).await?;
		if saved
			.subjects
			.iter()
			.any(|s| !crate::authorization::identity::enabled(&snapshot, s))
		{
			return Err(Error::Forbidden);
		}
		let pool = lease.tx().pool().clone();
		let client = crate::semantic::backend::client()?;
		let mut access = crate::authorization::access::Access::from_transaction(
			lease.tx().lend(),
			&pool,
			&destination.home,
			client,
			&identity,
			snapshot,
		);
		access.subjects = saved.subjects;
		access.worker();
		let mut publisher = PublisherTransaction {
			access,
			target: lease.tx(),
		};
		let result = Box::pin(check_source(
			&mut Lease::Inherited(&mut publisher.access),
			&destination,
			source_id,
			source_revision,
			max_visits,
			visits,
		))
		.await;
		let audit = publisher.access.flush_audit().await;
		drop(publisher);
		audit?;
		result
	} else {
		if saved.subject != "operator" || !saved.tenant.is_empty() || !saved.subjects.is_empty() {
			return Err(Error::Forbidden);
		}
		Box::pin(check_source(
			&mut Lease::BorrowedOperator(lease.tx()),
			&destination,
			source_id,
			source_revision,
			max_visits,
			visits,
		))
		.await
	}
}
struct PublisherTransaction<'a> {
	access: crate::authorization::access::Access,
	target: &'a mut native::Transaction,
}
impl Drop for PublisherTransaction<'_> {
	fn drop(&mut self) {
		self.target.restore(self.access.return_transaction());
	}
}

async fn check_source(
	lease: &mut Lease<'_>,
	destination: &Bank,
	id: Uuid,
	revision: i64,
	max_visits: usize,
	visits: &mut usize,
) -> Result<()> {
	let source = units::load(lease, id, false)
		.await?
		.ok_or(Error::Forbidden)?;
	if source.bank.workspace != destination.workspace
		|| source.bank.home != destination.home
		|| source.bank.tenant != destination.tenant
		|| source.bank.participant.is_none()
		|| source.revision != revision
		|| !source.visible()
	{
		return Err(Error::Conflict("published memory source changed".into()));
	}
	units::authorize(lease, &source.bank, "memory.publish").await?;
	Box::pin(units::current_in(
		lease,
		destination.workspace,
		&[source.evidence()],
		max_visits,
		visits,
	))
	.await
}

pub(crate) async fn publish(
	lease: &mut Lease<'_>,
	source: &Evidence,
	mutation: &Mutation,
	bounds: &Bounds,
) -> Result<Vec<Unit>> {
	human(lease)?;
	let Evidence::Unit { bank, id, revision } = source else {
		return Err(Error::Invalid(
			"publication requires an exact admitted unit".into(),
		));
	};
	if bank.participant.is_none()
		|| mutation.bank.participant.is_some()
		|| bank.home != mutation.bank.home
		|| bank.tenant != mutation.bank.tenant
		|| bank.workspace != mutation.bank.workspace
		|| mutation.changes.len() != 1
	{
		return Err(Error::Invalid(
			"publication selects one private unit into its Workspace bank".into(),
		));
	}
	repository::lock_workspace(lease, bank.workspace, true).await?;
	units::authorize(lease, bank, "memory.publish").await?;
	units::authorize(lease, &mutation.bank, "memory.write").await?;
	units::current(
		lease,
		bank.workspace,
		std::slice::from_ref(source),
		bounds.max_graph_visits,
	)
	.await?;
	let original = units::load(lease, *id, false)
		.await?
		.ok_or(Error::Forbidden)?;
	let destination = repository::bank_id(lease, &mutation.bank, true)
		.await?
		.ok_or(Error::Forbidden)?;
	let grant = mutation.operation_id;
	if let Some(existing) = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_publications"))
			.and_where(Expr::col("id").eq(Expr::value(grant)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	{
		if existing.try_get::<Uuid>("bank_id")? != destination
			|| existing.try_get::<Uuid>("source_id")? != *id
			|| existing.try_get::<i64>("source_revision")? != *revision
		{
			return Err(Error::Conflict("publication ID was reused".into()));
		}
	} else {
		let authority = lease.saved()?;
		native::query(
			&Query::insert()
				.into_table(Alias::new("memory_publications"))
				.columns(
					[
						"id",
						"bank_id",
						"source_id",
						"source_revision",
						"revision",
						"authority",
						"deleted",
						"created_at",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(grant))
						.expr(Expr::value(destination))
						.expr(Expr::value(*id))
						.expr(Expr::value(*revision))
						.expr(Expr::value(1_i64))
						.expr(Expr::value(authority))
						.expr(Expr::value(false))
						.expr(Expr::value(Utc::now()))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
	}
	let mut selected = mutation.clone();
	let target = match &mut selected.changes[0] {
		Change::Add { content, .. } | Change::Correct { content, .. } => content,
		Change::Delete { .. } => {
			return Err(Error::Invalid("publication cannot delete a unit".into()));
		}
	};
	// The caller selects the unit, not arbitrary declassified replacements.
	if *target != original.content {
		return Err(Error::Conflict(
			"publication content differs from the observed source".into(),
		));
	}
	target.evidence = vec![Evidence::Publication {
		id: grant,
		revision: 1,
	}];
	target.links.clear();
	repository::mutate(lease, &selected, bounds).await
}
