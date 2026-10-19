//! Purging deleted Tenants (ADR 0061). A resumable job on the hourly retention
//! loop: it purges the Tenants whose grace period has ended, and deletes the
//! audit history that outlived an earlier purge.
//!
//! A purge runs its steps in order and records the step it reached, so a
//! failed step is retried from itself at the next sweep. Every step is
//! idempotent. Provider Credentials go first, because their Key Material lives
//! outside the database where nothing but the rows names it, and capability
//! object files go before the `core_objects` rows that name them.
use crate::Result;
use crate::apps::identity::models::{TenantDeletion, database_time};
use crate::apps::identity::repositories::tenant_purge;
use crate::database::{classification, native, purge};
use crate::store::Store;
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::identity::tenancy::deletion::PurgeStep;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

/// The actor recorded for what a purge does.
const ACTOR: &str = "tenant-purge";
/// Capability objects erased per transaction.
const OBJECT_BATCH: u64 = 100;

fn scope() -> TenancyScope {
	TenancyScope::Deployment(DeploymentScopeReason::TenantPurge)
}

/// Purges every Tenant whose grace period has ended, then deletes the audit
/// history whose period has ended. A Tenant whose purge fails is logged and
/// left for the next sweep, so one Tenant never holds up the others.
pub async fn sweep(store: &Store, audit_retention_days: u32) -> Result<()> {
	for deletion in claim(store).await? {
		let tenant = deletion.tenant.clone();
		if let Err(error) = purge_tenant(store, deletion).await {
			tracing::warn!(%tenant, %error, "a Tenant purge failed and will resume at its failed step");
		}
	}
	erase_audit(store, audit_retention_days).await
}

/// Leases the deletions that are due to this caller. Whoever holds the lease
/// purges; the others skip the Tenant until the lease runs out.
pub async fn claim(store: &Store) -> Result<Vec<TenantDeletion>> {
	store
		.pool
		.atomic(&scope(), async |tx| tenant_purge::claim(tx).await)
		.await
}

/// Runs the remaining steps of one claimed purge. A failure is recorded on the
/// deletion, with the step, before it is returned.
pub async fn purge_tenant(store: &Store, deletion: TenantDeletion) -> Result<()> {
	let mut step = match deletion.purge_step.as_deref() {
		Some(step) => PurgeStep::parse(step)?,
		None => PurgeStep::FIRST,
	};
	loop {
		if let Err(error) = run(store, &deletion, step).await {
			let message = error.to_string();
			store
				.pool
				.atomic(&scope(), async |tx| {
					tenant_purge::fail(tx, &deletion, step, &message).await
				})
				.await?;
			return Err(error);
		}
		let Some(next) = step.next() else {
			return Ok(());
		};
		store
			.pool
			.atomic(&scope(), async |tx| {
				tenant_purge::advance(tx, &deletion, next).await
			})
			.await?;
		step = next;
	}
}

/// Runs one step. Every step works while a transaction holds the deletion row
/// (see [`tenant_purge::hold`]), so a restore can neither land between the
/// check and the step nor interleave with it, and a restored, purged or taken
/// over Tenant makes the step refuse before it deletes anything.
async fn run(store: &Store, deletion: &TenantDeletion, step: PurgeStep) -> Result<()> {
	match step {
		PurgeStep::ProviderCredentials => {
			store
				.pool
				.atomic(&scope(), async |tx| {
					tenant_purge::hold(tx, deletion).await?;
					provider_credentials(store, &deletion.tenant).await
				})
				.await
		}
		PurgeStep::CapabilityObjects => {
			store
				.pool
				.atomic(&scope(), async |tx| {
					tenant_purge::hold(tx, deletion).await?;
					capability_objects(store, &deletion.tenant).await
				})
				.await
		}
		PurgeStep::Rows => rows(store, deletion).await,
		PurgeStep::Finish => {
			store
				.pool
				.atomic(&scope(), async |tx| {
					tenant_purge::hold(tx, deletion).await?;
					tenant_purge::finish(tx, deletion).await
				})
				.await
		}
	}
}

/// Ends the Tenant's Plan Connections, if deletion could not, and revokes and
/// deletes its Provider Credentials with their stored Key Material.
async fn provider_credentials(store: &Store, tenant: &str) -> Result<()> {
	if let Some(service) = &store.plan_connections {
		service.retire_tenant(tenant, ACTOR).await?;
	}
	if let Some(service) = &store.provider_credentials {
		service.purge_tenant(tenant, ACTOR).await?;
	}
	Ok(())
}

/// Erases the Tenant's capability object files and the `core_objects` rows
/// that name them.
async fn capability_objects(store: &Store, tenant: &str) -> Result<()> {
	loop {
		let mut tx = store.pool.begin(&scope()).await?;
		let ids: Vec<Uuid> = {
			let tx = &mut tx;
			native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_objects"))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
					.limit(OBJECT_BATCH)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_all(&mut **tx)
			.await?
		};
		if ids.is_empty() {
			return Ok(());
		}
		for id in ids {
			store
				.capabilities
				.erase_committed(&mut tx, tenant, id)
				.await?;
		}
		tx.commit().await?;
	}
}

/// Deletes the Tenant's rows of every purged table, children before parents.
async fn rows(store: &Store, deletion: &TenantDeletion) -> Result<()> {
	let tables = store
		.pool
		.atomic(&scope(), async |tx| {
			let references = purge::references(tx).await?;
			purge::deletion_order(
				classification::purged(),
				&references,
				classification::UNLINKED,
			)
		})
		.await?;
	store
		.pool
		.atomic(&scope(), async |tx| {
			tenant_purge::hold(tx, deletion).await?;
			purge::unlink(tx, classification::UNLINKED, &deletion.tenant).await
		})
		.await?;
	for table in tables {
		store
			.pool
			.atomic(&scope(), async |tx| {
				tenant_purge::hold(tx, deletion).await?;
				purge::delete_tenant_rows(tx, table, &deletion.tenant).await?;
				tenant_purge::renew(tx, deletion).await
			})
			.await?;
	}
	Ok(())
}

/// Deletes the audit history of Tenants purged more than `retention_days` ago.
pub async fn erase_audit(store: &Store, retention_days: u32) -> Result<()> {
	let due = store
		.pool
		.atomic(&scope(), async |tx| {
			let now = database_time(tx).await?;
			tenant_purge::audit_due(tx, now, retention_days).await
		})
		.await?;
	for deletion in due {
		store
			.pool
			.atomic(&scope(), async |tx| {
				let references = purge::references(tx).await?;
				for table in
					purge::deletion_order(classification::audit_history(), &references, &[])?
				{
					purge::delete_tenant_rows(tx, table, &deletion.tenant).await?;
				}
				tenant_purge::audit_erased(tx, deletion.id).await
			})
			.await?;
	}
	Ok(())
}
