//! Persistent dashboard_execution_origins records.

use super::AuthorizationExecution;
use crate::Result;
use crate::apps::execution::models::Run;
use crate::apps::execution::services::states::RunControl;
use crate::apps::identity::models::DashboardMapping;
use crate::authorization::identity::SubjectIdentity;
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, OrmExecutor, QuerySet};
use reinhardt::model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

type StoredOriginBinding = (String, Uuid, Uuid, Option<Uuid>, Option<i64>, Option<i64>);

#[model(app_label = "identity", table_name = "dashboard_execution_origins")]
#[derive(Serialize, Deserialize)]
pub struct DashboardExecutionOrigin {
	#[field(primary_key = true)]
	pub run_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		db_constraint = "dashboard_execution_origins_identity_id_fkey",
		db_index = false
	)]
	pub identity: ForeignKeyField<crate::apps::identity::models::DashboardIdentity>,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "dashboard_execution_origins_mapping_id_fkey",
		db_index = false
	)]
	pub mapping: ForeignKeyField<crate::apps::identity::models::DashboardMapping>,
	#[field(null = true)]
	pub account_id: Option<Uuid>,
	#[field(null = true)]
	pub suspension_epoch: Option<i64>,
	#[field(null = true)]
	pub source_disable_epoch: Option<i64>,
}

impl DashboardExecutionOrigin {
	/// A product-local retirement must survive restoring local eligibility even
	/// when the shared authority never observed that local policy withdrawal.
	pub(crate) async fn pause_local_retirement(
		db: &mut dyn TransactionExecutor,
		identity: Uuid,
	) -> Result<()> {
		let saved = Query::select()
			.column(Alias::new("run_id"))
			.from(Alias::new("dashboard_execution_origins"))
			.and_where(Expr::col("identity_id").eq(Expr::value(identity)))
			.to_owned();
		let legacy = Query::select()
			.column((Alias::new("e"), Alias::new("run_id")))
			.from_as(Alias::new("authorization_execution"), Alias::new("e"))
			.join(
				reinhardt::query::JoinType::InnerJoin,
				reinhardt::query::TableRef::table_alias(
					Alias::new("authorization_credentials"),
					Alias::new("c"),
				),
				Expr::col((Alias::new("e"), Alias::new("credential_id")))
					.equals((Alias::new("c"), Alias::new("id"))),
			)
			.join(
				reinhardt::query::JoinType::InnerJoin,
				reinhardt::query::TableRef::table_alias(
					Alias::new("dashboard_mappings"),
					Alias::new("m"),
				),
				reinhardt::query::Condition::any()
					.add(
						Expr::col((Alias::new("m"), Alias::new("credential_id")))
							.equals((Alias::new("c"), Alias::new("id"))),
					)
					.add(
						Expr::col((Alias::new("m"), Alias::new("id")))
							.equals((Alias::new("c"), Alias::new("mapping_id"))),
					),
			)
			.and_where(
				Expr::col((Alias::new("m"), Alias::new("identity_id"))).eq(Expr::value(identity)),
			)
			.and_where(
				Expr::col((Alias::new("e"), Alias::new("tenant")))
					.equals((Alias::new("m"), Alias::new("tenant"))),
			)
			.and_where(
				Expr::col((Alias::new("e"), Alias::new("root_subject")))
					.equals((Alias::new("m"), Alias::new("subject"))),
			)
			.and_where(
				Expr::col((Alias::new("e"), Alias::new("run_id"))).not_in_subquery(
					Query::select()
						.column(Alias::new("run_id"))
						.from(Alias::new("dashboard_execution_origins"))
						.to_owned(),
				),
			)
			.to_owned();
		// This is an upgraded control-plane write, including when a legacy
		// worker's lease is fenced by the input ledger. Keep the lease so an
		// already-admitted short operation may finish; the next boundary sees
		// PAUSED, and its stale Run revision cannot overwrite this stop.
		Run::worker_context(db).await?;
		let update = Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("control"), "PAUSED")
			.value(Alias::new("error"), "external identity disabled")
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(
				reinhardt::query::Condition::any()
					.add(Expr::col("id").in_subquery(saved))
					.add(Expr::col("id").in_subquery(legacy)),
			)
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.and_where(Expr::col("control").ne(Expr::value("CANCELLED")))
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&update).execute(db).await?;
		Ok(())
	}

	pub(crate) async fn is_human_origin(
		db: &mut dyn TransactionExecutor,
		run: Uuid,
	) -> Result<bool> {
		Ok(Self::saved_or_legacy(db, run).await?.is_some())
	}
	async fn saved_or_legacy(
		db: &mut dyn TransactionExecutor,
		run: Uuid,
	) -> Result<Option<(Self, bool)>> {
		let query = Query::select()
			.columns(
				[
					"tenant",
					"identity_id",
					"mapping_id",
					"account_id",
					"suspension_epoch",
					"source_disable_epoch",
				]
				.map(Alias::new),
			)
			.from(Alias::new("dashboard_execution_origins"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder);
		let saved: Option<StoredOriginBinding> = crate::database::native::query_as(&query)
			.columns(&[
				"tenant",
				"identity_id",
				"mapping_id",
				"account_id",
				"suspension_epoch",
				"source_disable_epoch",
			])
			.fetch_optional(&mut *db)
			.await?;
		if let Some((
			tenant,
			identity,
			mapping,
			account_id,
			suspension_epoch,
			source_disable_epoch,
		)) = saved
		{
			return Ok(Some((
				Self::build()
					.run_id(run)
					.tenant(tenant)
					.identity(identity)
					.mapping(mapping)
					.account_id(account_id)
					.suspension_epoch(suspension_epoch)
					.source_disable_epoch(source_disable_epoch)
					.finish(),
				true,
			)));
		}
		// Before common-auth, user-issued credentials could retain a Mapping
		// without a separate dashboard origin row. Reconstruct provenance, but
		// never silently assign today's suspension epoch to that old work.
		let query = Query::select()
			.columns(["tenant", "root_subject", "credential_id"].map(Alias::new))
			.from(Alias::new("authorization_execution"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder);
		let grant: Option<(String, String, Uuid)> = crate::database::native::query_as(&query)
			.columns(&["tenant", "root_subject", "credential_id"])
			.fetch_optional(&mut *db)
			.await?;
		let Some((tenant, subject, credential)) = grant else {
			return Ok(None);
		};
		let query = Query::select()
			.columns(["id", "identity_id", "tenant", "subject"].map(Alias::new))
			.from(Alias::new("dashboard_mappings"))
			.and_where(
				reinhardt::query::Condition::any()
					.add(Expr::col("credential_id").eq(Expr::value(credential)))
					.add(
						Expr::col("id").in_subquery(
							Query::select()
								.column(Alias::new("mapping_id"))
								.from(Alias::new("authorization_credentials"))
								.and_where(Expr::col("id").eq(Expr::value(credential)))
								.to_owned(),
						),
					),
			)
			.to_string(PostgresQueryBuilder);
		let mapping: Option<(Uuid, Uuid, String, String)> =
			crate::database::native::query_as(&query)
				.columns(&["id", "identity_id", "tenant", "subject"])
				.fetch_optional(&mut *db)
				.await?;
		let Some((mapping, identity, mapping_tenant, mapping_subject)) = mapping else {
			return Ok(None);
		};
		if tenant != mapping_tenant || subject != mapping_subject {
			return Err(crate::Error::Forbidden);
		}
		Ok(Some((
			Self::build()
				.run_id(run)
				.tenant(tenant)
				.identity(identity)
				.mapping(mapping)
				.account_id(None)
				.suspension_epoch(None)
				.source_disable_epoch(None)
				.finish(),
			false,
		)))
	}
	/// Every worker boundary retains its original common-account suspension
	/// epoch. A completed suspension cannot disappear behind a later restore.
	pub(crate) async fn require_current(
		db: &mut dyn TransactionExecutor,
		run: Uuid,
		policy: Option<&aidash_application::ports::authorization::dashboard::AccountPolicy>,
	) -> Result<()> {
		let Some(authority) = policy.and_then(|policy| policy.authority.as_ref()) else {
			return Ok(());
		};
		let Some((origin, _)) = Self::saved_or_legacy(db, run).await? else {
			return Ok(());
		};
		let epoch = origin
			.suspension_epoch
			.map(u64::try_from)
			.transpose()
			.map_err(|_| crate::Error::Forbidden)?;
		let source_epoch = origin
			.source_disable_epoch
			.map(u64::try_from)
			.transpose()
			.map_err(|_| crate::Error::Forbidden)?;
		authority
			.validate_bound_origin(&aidash_domain::accounts::DurableOrigin {
				identity_id: origin.identity_id(),
				account_id: origin.account_id,
				suspension_epoch: epoch,
				source_disable_epoch: source_epoch,
			})
			.await
			.map_err(Into::into)
	}

	pub(crate) async fn renew_manual(
		db: &mut dyn TransactionExecutor,
		run: Uuid,
		identity: &SubjectIdentity,
		policy: Option<&aidash_application::ports::authorization::dashboard::AccountPolicy>,
	) -> Result<()> {
		let Some(authority) = policy.and_then(|policy| policy.authority.as_ref()) else {
			return Ok(());
		};
		let Some((mut origin, saved)) = Self::saved_or_legacy(db, run).await? else {
			return Ok(());
		};
		// Manual recovery cannot change to another linked identity, or use a
		// durable bearer credential in place of post-suspension interactive login.
		let session = identity
			.http_session
			.as_ref()
			.ok_or(crate::Error::Forbidden)?;
		if session.identity_id != origin.identity_id() {
			return Err(crate::Error::Forbidden);
		}
		authority
			.validate_interactive(
				session.identity_id,
				session.provider_session.ok_or(crate::Error::Unauthorized)?,
			)
			.await?;
		let binding = authority.origin_binding(origin.identity_id()).await?;
		origin.account_id = binding.account_id;
		origin.suspension_epoch = binding
			.suspension_epoch
			.map(i64::try_from)
			.transpose()
			.map_err(|_| crate::Error::IdentityStatusUnavailable)?;
		origin.source_disable_epoch = binding
			.source_disable_epoch
			.map(i64::try_from)
			.transpose()
			.map_err(|_| crate::Error::IdentityStatusUnavailable)?;
		let query = if saved {
			Query::update()
				.table(Alias::new("dashboard_execution_origins"))
				.value(Alias::new("account_id"), origin.account_id)
				.value(Alias::new("suspension_epoch"), origin.suspension_epoch)
				.value(
					Alias::new("source_disable_epoch"),
					origin.source_disable_epoch,
				)
				.and_where(Expr::col("run_id").eq(Expr::value(run)))
				.to_string(PostgresQueryBuilder)
		} else {
			Query::insert()
				.into_table(Alias::new("dashboard_execution_origins"))
				.columns(
					[
						"run_id",
						"tenant",
						"identity_id",
						"mapping_id",
						"account_id",
						"suspension_epoch",
						"source_disable_epoch",
					]
					.map(Alias::new),
				)
				.values_panic([
					reinhardt::query::IntoValue::into_value(run),
					origin.tenant.clone().into(),
					origin.identity_id().into(),
					origin.mapping_id().into(),
					reinhardt::query::IntoValue::into_value(origin.account_id),
					reinhardt::query::IntoValue::into_value(origin.suspension_epoch),
					reinhardt::query::IntoValue::into_value(origin.source_disable_epoch),
				])
				.to_string(PostgresQueryBuilder)
		};
		crate::database::native::query(&query).execute(db).await?;
		Ok(())
	}
	pub(crate) async fn status_waiting<E: OrmExecutor>(
		db: &mut E,
		identity: Uuid,
	) -> Result<Vec<Uuid>> {
		Ok(Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.filter_in_subquery("run_id", |runs: QuerySet<Run>| {
				runs.filter(Run::field_control().eq(RunControl::Paused))
					.filter(Run::field_error().eq(Some("identity status unavailable".to_owned())))
					.values(&["id"])
			})?
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|row| row.run_id())
			.collect())
	}

	pub(crate) async fn original_subject<E: OrmExecutor>(
		db: &mut E,
		identity: Uuid,
		run: Uuid,
	) -> Result<Option<SubjectIdentity>> {
		let origin = Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.filter(Self::field_run_id().eq(run))
			.all_with_db(db)
			.await?
			.pop();
		let Some(origin) = origin else {
			return Ok(None);
		};
		let Some(mapping) = DashboardMapping::find(db, origin.mapping_id()).await? else {
			return Ok(None);
		};
		let original = AuthorizationExecution::objects()
			.filter(AuthorizationExecution::field_run_id().eq(run))
			.all_with_db(db)
			.await?
			.pop();
		let Some(original) = original else {
			return Ok(None);
		};
		if original.tenant != mapping.tenant || original.root_subject != mapping.subject {
			return Err(crate::Error::Forbidden);
		}
		Ok(Some(SubjectIdentity {
			http_session: None,
			credential_id: original.credential_id(),
			tenant: mapping.tenant,
			subject: mapping.subject,
		}))
	}
}

#[cfg(test)]
#[path = "../tests/common_origin_epochs.rs"]
mod tests;
