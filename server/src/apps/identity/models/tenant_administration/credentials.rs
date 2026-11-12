//! Subject Credentials a member issues for its own subject, and those its
//! Tenant issues for service and agent subjects (#358, ADR 0063).
use super::{TenantAdministrator, rules};
use crate::apps::identity::models::{AuthorizationCredential, DashboardMapping};
use crate::apps::identity::serializers::identity::{Credential, IssuedCredential};
use crate::authorization::Snapshot;
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_domain::identity::credentials;
use aidash_domain::policy::SubjectKind;
use aidash_domain::tenancy::TenantName;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{AtomicTransaction, Model};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

/// Advisory lock namespace serializing the credentials one Mapping issues.
const ISSUE_LOCK: i64 = 36_300_358;

async fn lock_issues(tx: &mut AtomicTransaction, mapping: Uuid) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					Expr::value(mapping.to_string()).into(),
					Expr::value(ISSUE_LOCK).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
	Ok(())
}

impl TenantAdministrator {
	pub(crate) async fn set_credential_maximum(
		&self,
		db: &Pool,
		seconds: i64,
		deployment: i64,
	) -> Result<()> {
		use reinhardt::db::orm::query::FieldAssignment;
		let action = aidash_domain::identity::tenancy::TENANT_UPDATE;
		let may_change = self.may_change(db, action).await?;
		db.atomic(&self.scope()?, async |tx| {
			let waiting = if may_change {
				Some(crate::database::authority_change::raise(tx, self.tenant()).await?)
			} else {
				None
			};
			let held = self.lock_change(tx, may_change, action).await;
			let snapshot = if let Some(waiting) = waiting {
				waiting.settle(tx, held).await?
			} else {
				held?
			};
			self.require(tx, &snapshot, action, self.tenant(), json!({}))
				.await?;
			let old = super::super::Tenant::credential_maximum(tx, self.tenant(), deployment, true)
				.await?;
			let assignments: Vec<FieldAssignment> = vec![
				(
					super::super::Tenant::field_credential_max_seconds(),
					Some(seconds),
				)
					.into(),
			];
			super::super::Tenant::objects()
				.filter(super::super::Tenant::field_name().eq(self.tenant().to_owned()))
				.update_fields_with_conn(tx, assignments)
				.await?;
			if seconds < old {
				let now = super::super::dashboard_administration::database_time(tx).await?;
				super::super::authority::authority_control(tx).await?;
				let rows = AuthorizationCredential::objects()
					.filter(AuthorizationCredential::field_tenant().eq(self.tenant().to_owned()))
					.filter(AuthorizationCredential::field_mapping_id().is_null())
					.filter(AuthorizationCredential::field_issued_by().ne("dashboard-oidc"))
					.filter(AuthorizationCredential::field_revoked_at().is_null())
					.filter(AuthorizationCredential::field_expires_at().gt(now))
					.select_for_update()
					.all_with_executor(tx)
					.await
					.map_err(reinhardt::core::exception::Error::from)?;
				for mut row in rows {
					let shortened = row.created_at + chrono::Duration::seconds(seconds);
					if shortened < row.expires_at {
						row.expires_at = shortened;
						row.end_reason = Some("lowered_maximum".into());
						AuthorizationCredential::objects()
							.update_with_conn(tx, &row)
							.await?;
					}
				}
			}
			Ok(())
		})
		.await
	}
	/// Requires that this subject could issue a credential for `subject` now:
	/// a service or agent subject that no Mapping ever named, under
	/// `credential.issue`. A credential for a person's subject would act as that
	/// person in every decision and audit.
	pub(crate) async fn require_issuable(
		&self,
		tx: &mut dyn TransactionExecutor,
		snapshot: &Snapshot,
		subject: &str,
	) -> Result<()> {
		let kind = snapshot
			.bundle
			.subjects
			.get(subject)
			.map(|entry| entry.kind.clone());
		self.require(
			tx,
			snapshot,
			rules::CREDENTIAL_ISSUE,
			subject,
			json!({"kind": kind}),
		)
		.await?;
		if !matches!(kind, Some(SubjectKind::Service | SubjectKind::Agent)) {
			return Err(Error::Invalid(
				"credentials for another subject are issued only for service and agent subjects"
					.into(),
			));
		}
		let (sql, values) = Query::select()
			.expr(Expr::value(1_i32))
			.from(Alias::new(DashboardMapping::table_name()))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(self.tenant().to_owned())))
			.and_where(Expr::col(Alias::new("subject")).eq(Expr::value(subject.to_owned())))
			.limit(1)
			.build(PostgresQueryBuilder);
		if tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_some()
		{
			return Err(Error::Invalid(
				"a person was mapped to this subject, so only that person issues its credentials"
					.into(),
			));
		}
		Ok(())
	}

	/// Issues a credential for this member's own subject, bound to its
	/// Mapping, or one the Tenant holds for a service or agent subject that no
	/// person was ever mapped to. A credential never issues another, because
	/// only a browser Mapping acts here.
	pub(crate) async fn issue_credential(
		&self,
		db: &Pool,
		subject: &str,
		lifetime: i64,
		deployment_maximum: i64,
	) -> Result<IssuedCredential> {
		aidash_domain::policy::identifier(subject)?;
		let token = credentials::subject_token(&TenantName::new(self.tenant())?);
		let own = subject == self.identity.subject;
		let credential = db
			.atomic(&self.scope()?, async |tx| {
				let snapshot = self.lock(tx, false).await?;
				if own {
					credentials::validate_lifetime(lifetime)?;
					self.require(tx, &snapshot, rules::CREDENTIAL_CREATE, subject, json!({}))
						.await?;
					lock_issues(tx, self.mapping_id).await?;
					// Expiry follows the database clock, as issuance and authentication do.
					let now = super::super::dashboard_administration::database_time(tx).await?;
					let active = AuthorizationCredential::objects()
						.filter(
							AuthorizationCredential::field_mapping_id().eq(Some(self.mapping_id)),
						)
						.filter(AuthorizationCredential::field_parent_id().is_null())
						.filter(AuthorizationCredential::field_revoked_at().is_null())
						.filter(AuthorizationCredential::field_expires_at().gt(now))
						.all_with_db(tx)
						.await?
						.len();
					if active >= credentials::MAX_ACTIVE_PER_MAPPING {
						return Err(Error::Conflict(format!(
							"a Mapping holds at most {} active credentials",
							credentials::MAX_ACTIVE_PER_MAPPING
						)));
					}
				} else {
					self.require_issuable(tx, &snapshot, subject).await?;
					let maximum = super::super::Tenant::credential_maximum(
						tx,
						self.tenant(),
						deployment_maximum,
						false,
					)
					.await?;
					credentials::validate_tenant_lifetime(lifetime, maximum)?;
				}
				if !crate::authorization::identity::enabled(&snapshot, subject) {
					return Err(Error::Invalid(
						"credential subject must exist and be enabled, including its delegators"
							.into(),
					));
				}
				AuthorizationCredential::issue(
					tx,
					self.tenant(),
					subject,
					crate::authorization::identity::digest(&token),
					lifetime,
					&self.revision_actor(),
					own.then_some(self.mapping_id),
				)
				.await
			})
			.await?;
		Ok(IssuedCredential { credential, token })
	}

	/// One page of the Tenant's credentials under `credential.read`, or only
	/// those this member issued through its Mapping.
	pub(crate) async fn credentials(
		&self,
		db: &Pool,
		offset: u64,
		mine: bool,
		active: bool,
	) -> Result<Vec<Credential>> {
		db.atomic(&self.scope()?, async |tx| {
			let snapshot = self.lock(tx, false).await?;
			if !mine {
				self.require(
					tx,
					&snapshot,
					rules::CREDENTIAL_READ,
					rules::COLLECTION,
					json!({}),
				)
				.await?;
			}
			AuthorizationCredential::visible_page(
				tx,
				self.tenant(),
				mine.then_some(self.mapping_id),
				offset,
				active,
			)
			.await
		})
		.await
	}

	/// Revokes a credential this member issued through its Mapping, or any of
	/// the Tenant's under `credential.revoke`.
	pub(crate) async fn revoke_credential(&self, db: &Pool, id: Uuid) -> Result<Credential> {
		let may_revoke = self.may_change(db, rules::CREDENTIAL_REVOKE).await?;
		db.atomic(&self.scope()?, async |tx| {
			// Revoking waits for the work holding the credential, so it is an
			// Authority Change, except for someone this is about to refuse.
			if !may_revoke
				&& !AuthorizationCredential::issued_through(tx, self.tenant(), id, self.mapping_id)
					.await?
			{
				let snapshot = self.lock(tx, false).await?;
				self.require(
					tx,
					&snapshot,
					rules::CREDENTIAL_REVOKE,
					&id.to_string(),
					json!({}),
				)
				.await?;
				return Err(Error::Forbidden);
			}
			let waiting = crate::database::authority_change::raise(tx, self.tenant()).await?;
			let held = match self.lock(tx, false).await {
				Ok(snapshot) => AuthorizationCredential::exportable(tx, self.tenant(), id)
					.await
					.map(|found| (snapshot, found)),
				Err(error) => Err(error),
			};
			let (snapshot, found) = waiting.settle(tx, held).await?;
			if found
				.as_ref()
				.is_none_or(|credential| credential.mapping_id != Some(self.mapping_id))
			{
				self.require(
					tx,
					&snapshot,
					rules::CREDENTIAL_REVOKE,
					&id.to_string(),
					json!({}),
				)
				.await?;
			}
			AuthorizationCredential::revoke_in(tx, self.tenant(), id).await
		})
		.await
	}
}
