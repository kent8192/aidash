//! Spending Demo Reservations (ADR 0079): the only path from a verified email
//! address to a Mapping. Sign-in and loading Demo data both end here, so a
//! Reservation is spent under the same locks and checks as a registration
//! approval, without the Registration Request.

use super::dashboard_administration::{database_time, issue_mapping, lock_policy};
use super::{
	AdministrationActor, AdministrationEvent, DashboardIdentity, DashboardMapping, DemoReservation,
};
use crate::apps::identity::repositories::plan_connections::refused_shared_holder;
use crate::apps::identity::services::dashboard_rules;
use crate::apps::identity::services::oidc::{digest, random_secret};
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::ports::authorization::dashboard::AccountPolicy;
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::query::FieldAssignment;
use reinhardt::db::orm::{AtomicTransaction, Model};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use uuid::Uuid;

/// The administration history action of a spent Reservation.
pub(crate) const DEMO_RESERVATION_FULFIL: &str = "demo_reservation.fulfil";

/// What spending one Reservation came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Fulfilment {
	/// The External Identity was mapped, and the Reservation is spent.
	Mapped(Uuid),
	/// The External Identity already held the Mapping, so the Reservation is
	/// spent without adding one.
	AlreadyMapped,
	/// The Reservation is left as it was: it cannot be spent now.
	Skipped(Skip),
}

/// Why a Reservation could not be spent. None of them is an error: sign-in
/// goes on, and a later sign-in or load may still spend it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
	/// Another External Identity spent it, or it is gone.
	Spent,
	TenantDeleted,
	/// The subject no longer exists, or is no longer an enabled user subject.
	SubjectUnavailable,
	/// The subject holds a Plan Connection (ADR 0007).
	SharedHolder,
	/// The External Identity is gone or disabled.
	IdentityUnavailable,
	/// The External Identity's issuer is refused, or has a Tenant Binding.
	Restricted,
}

impl DemoReservation {
	/// Holds the Reservation of `email` for `subject` in `tenant`, which
	/// loading Demo data does for each person (ADR 0079), and spends it at once
	/// when exactly one enabled External Identity already holds `email` as its
	/// verified address and its issuer has no Tenant Binding. It runs in the
	/// Tenant's scope in one transaction. A spent Reservation is left alone, as
	/// is one whose address more than one identity holds: the first of them to
	/// sign in again, or a later load, spends it. Without a sign-in `policy`
	/// nobody can have signed in, and a Binding is known only from the policy,
	/// so the Reservation is only held.
	pub async fn provide(
		db: &Pool,
		policy: Option<&AccountPolicy>,
		tenant: &str,
		email: &str,
		subject: &str,
	) -> Result<()> {
		let email = email.to_lowercase();
		let scope = TenancyScope::tenant(tenant)?;
		db.atomic(&scope, async |tx| {
			let reservation = Self::hold(tx, tenant, &email, subject).await?;
			if reservation.spent_at.is_some() || policy.is_none() {
				return Ok(());
			}
			// Found before any lock, because a sign-in takes the identity
			// lock before the Reservation's.
			if let [(identity, true)] = holders(tx, &email).await?[..] {
				Self::fulfil_in(tx, policy, tenant, reservation.id, identity).await?;
			}
			Ok(())
		})
		.await
	}

	/// The unspent Reservations held for `email`, oldest first, across every
	/// Tenant. `email` is lowercased.
	pub(crate) async fn unspent(db: &Pool, email: &str) -> Result<Vec<Self>> {
		let scope = TenancyScope::Deployment(DeploymentScopeReason::DemoData);
		db.atomic(&scope, async |tx| {
			Ok(Self::objects()
				.filter(Self::field_email().eq(email.to_owned()))
				.filter(Self::field_spent_at().is_null())
				.order_by(&["created_at", "id"])
				.all_with_db(tx)
				.await?)
		})
		.await
	}

	/// The Reservation of `email` in `tenant`, or a new unspent one for
	/// `subject`. A Reservation never changes its subject: loading only adds.
	pub(crate) async fn hold(
		tx: &mut AtomicTransaction,
		tenant: &str,
		email: &str,
		subject: &str,
	) -> Result<Self> {
		let existing = Self::objects()
			.filter(Self::field_tenant().eq(tenant.to_owned()))
			.filter(Self::field_email().eq(email.to_owned()))
			.all_with_db(tx)
			.await?
			.pop();
		if let Some(existing) = existing {
			return Ok(existing);
		}
		let now = database_time(tx).await?;
		let reservation = Self::build()
			.id(Uuid::new_v4())
			.tenant(tenant.to_owned())
			.email(email.to_owned())
			.subject(subject.to_owned())
			.created_at(now)
			.spent_at(None)
			.spent_by(None)
			.finish();
		Ok(Self::objects().create_with_conn(tx, &reservation).await?)
	}

	/// Spends Reservation `id` of `tenant` on `identity`, in the Tenant's
	/// scope.
	pub(crate) async fn fulfil(
		db: &Pool,
		policy: Option<&AccountPolicy>,
		tenant: &str,
		id: Uuid,
		identity: Uuid,
	) -> Result<Fulfilment> {
		let scope = TenancyScope::tenant(tenant)?;
		db.atomic(&scope, async |tx| {
			Self::fulfil_in(tx, policy, tenant, id, identity).await
		})
		.await
	}

	/// Maps `identity` to the Reservation's subject and spends it, with the
	/// checks and writes of a registration approval: policy lock, deleted
	/// Tenant, identity lock, Tenant Binding, enabled user subject, no shared
	/// holder, then the credential and the Mapping. The caller is in
	/// `tenant`'s scope. A Reservation that cannot be spent is skipped, not
	/// failed, so a defect in Demo data never keeps a person from signing in;
	/// only a failure of the database is an error.
	pub(crate) async fn fulfil_in(
		tx: &mut AtomicTransaction,
		policy: Option<&AccountPolicy>,
		tenant: &str,
		id: Uuid,
		identity: Uuid,
	) -> Result<Fulfilment> {
		// Authority, here the policy, precedes the identity lock, then the
		// Reservation, as approval takes them.
		let snapshot = match lock_policy(tx, tenant, LockType::Share).await {
			Ok(snapshot) => snapshot,
			Err(Error::NotFound(_)) => return Ok(Fulfilment::Skipped(Skip::TenantDeleted)),
			Err(error) => return Err(error),
		};
		if crate::apps::identity::repositories::tenant_deletions::is_deleted(tx, tenant).await? {
			return Ok(Fulfilment::Skipped(Skip::TenantDeleted));
		}
		let Some(identity) = DashboardIdentity::objects()
			.filter(DashboardIdentity::field_id().eq(identity))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.filter(|identity| identity.disabled_at.is_none())
		else {
			return Ok(Fulfilment::Skipped(Skip::IdentityUnavailable));
		};
		// The Binding is checked on the row, not only at sign-in, because a
		// load maps identities that signed in earlier.
		if policy.is_some_and(|policy| !policy.unrestricted(&identity.issuer))
			|| crate::authorization::identity::require_dashboard_mapping(
				policy,
				&identity.issuer,
				tenant,
			)
			.is_err()
		{
			return Ok(Fulfilment::Skipped(Skip::Restricted));
		}
		let Some(reservation) = Self::objects()
			.filter(Self::field_id().eq(id))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.filter(|reservation| reservation.spent_at.is_none())
		else {
			return Ok(Fulfilment::Skipped(Skip::Spent));
		};
		if dashboard_rules::require_user_mapping(&snapshot, &reservation.subject).is_err() {
			return Ok(Fulfilment::Skipped(Skip::SubjectUnavailable));
		}
		let now = database_time(tx).await?;
		let held = DashboardMapping::objects()
			.filter(DashboardMapping::field_identity_id().eq(identity.id))
			.filter(DashboardMapping::field_tenant().eq(tenant.to_owned()))
			.filter(DashboardMapping::field_subject().eq(reservation.subject.clone()))
			.filter(DashboardMapping::field_enabled().eq(true))
			.exists_with_db(tx)
			.await?;
		let outcome = if held {
			Fulfilment::AlreadyMapped
		} else {
			match issue_mapping(
				tx,
				&snapshot,
				tenant,
				&reservation.subject,
				identity.id,
				digest(&random_secret()),
				now,
			)
			.await
			{
				Ok(mapping) => Fulfilment::Mapped(mapping),
				Err(error) if refused_shared_holder(&error) => {
					return Ok(Fulfilment::Skipped(Skip::SharedHolder));
				}
				Err(error) => return Err(error),
			}
		};
		let spent: Vec<FieldAssignment> = vec![
			(Self::field_spent_at(), Some(now)).into(),
			(Self::field_spent_by(), Some(identity.id)).into(),
		];
		Self::objects()
			.filter(Self::field_id().eq(reservation.id))
			.update_fields_with_conn(tx, spent)
			.await?;
		if let Fulfilment::Mapped(mapping) = &outcome {
			AdministrationActor::DemoData
				.record(
					tx,
					AdministrationEvent {
						action: DEMO_RESERVATION_FULFIL,
						identity: Some(identity.id),
						registration: None,
						mapping: Some(*mapping),
						tenant: Some(tenant),
						subject: Some(&reservation.subject),
						invitation: None,
					},
				)
				.await?;
		}
		Ok(outcome)
	}
}

/// Every External Identity holding `email` as its verified address, with
/// whether it is enabled.
async fn holders(tx: &mut AtomicTransaction, email: &str) -> Result<Vec<(Uuid, bool)>> {
	let lowered = SimpleExpr::FunctionCall(
		"lower".into_iden(),
		vec![Expr::col(Alias::new("verified_email")).into()],
	);
	let (sql, values) = Query::select()
		.column(Alias::new("id"))
		.expr_as(
			Expr::col(Alias::new("disabled_at")).is_null(),
			Alias::new("enabled"),
		)
		.from(Alias::new(DashboardIdentity::table_name()))
		.and_where(lowered.eq(Expr::value(email.to_owned())))
		.build(PostgresQueryBuilder);
	TransactionExecutor::fetch_all(tx, &sql, convert_values(values))
		.await?
		.iter()
		.map(|row| {
			Ok((
				row.get("id").map_err(FrameworkError::from)?,
				row.get("enabled").map_err(FrameworkError::from)?,
			))
		})
		.collect()
}
