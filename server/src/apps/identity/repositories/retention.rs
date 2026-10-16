//! A Tenant's own Retention Rules and their history (ADR 0045). A subject
//! acts through its Tenant policy in the authorization transaction; a
//! Deployment Administrator needs none.
use crate::apps::identity::models::{TenantRetentionRule, TenantRetentionRuleHistory};
use crate::authorization::{access::NativeAccess, identity::Actor};
use crate::store::Store;
use aidash_application::{
	Result,
	ports::retention::{RetentionRepository, RetentionScope},
};
use aidash_domain::retention::{self, Decider, RetentionChange, RetentionRules, SnapshotPeriods};
use async_trait::async_trait;
use chrono::Utc;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

/// Advisory lock namespace serializing changes of one Tenant's periods.
const LOCK_NAMESPACE: i64 = 71_003_238;

/// Hold `tenant`'s periods unchanged until the transaction ends: a change
/// takes the same lock exclusively. The retention sweep purges under it, so
/// it never purges by periods a Tenant has just lengthened or cleared.
pub(crate) async fn share_rules_lock(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> crate::Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock_shared".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					Expr::value(tenant).into(),
					Expr::value(LOCK_NAMESPACE).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

pub(crate) struct TenantRetention<'a> {
	pub store: &'a Store,
	pub actor: &'a Actor,
	/// The dashboard identity of a Deployment Administrator acting in a
	/// browser; none for the deployment's API token.
	pub administrator: Option<Uuid>,
	/// The Deployment's periods, also the longest a Tenant may keep.
	pub deployment: RetentionRules,
}

enum Transaction {
	Subject(Box<NativeAccess>),
	Administrator(Box<dyn TransactionExecutor>),
}

struct Scope {
	decider: Decider,
	deployment: RetentionRules,
	tx: Transaction,
}

fn native<T>(result: crate::Result<T>) -> Result<T> {
	result.map_err(Into::into)
}

fn seconds(value: u32) -> i32 {
	i32::try_from(value).unwrap_or(i32::MAX)
}

fn period(value: i32) -> u32 {
	u32::try_from(value).unwrap_or(0)
}

impl Scope {
	fn executor(&mut self) -> &mut dyn TransactionExecutor {
		match &mut self.tx {
			Transaction::Subject(access) => access.tenant_transaction().0,
			Transaction::Administrator(tx) => tx.as_mut(),
		}
	}
}

#[async_trait]
impl RetentionScope for Scope {
	fn decider(&self) -> Decider {
		self.decider.clone()
	}
	fn deployment(&self) -> RetentionRules {
		self.deployment
	}
	async fn require(&mut self, tenant: &str, action: &str) -> Result<()> {
		let Transaction::Subject(access) = &mut self.tx else {
			return Ok(());
		};
		let resource = access.resource(retention::RESOURCE, tenant, json!({}));
		native(access.require(&resource, action).await)
	}
	async fn stored(&mut self, tenant: &str) -> Result<Option<RetentionRules>> {
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![
						Expr::value(tenant).into(),
						Expr::value(LOCK_NAMESPACE).into(),
					],
				)],
			))
			.build(PostgresQueryBuilder);
		let tx = self.executor();
		native(
			tx.execute(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from),
		)?;
		let rows = native(
			TenantRetentionRule::objects()
				.filter(TenantRetentionRule::field_tenant().eq(tenant.to_owned()))
				.all_with_executor(tx)
				.await
				.map_err(|error| crate::Error::from(FrameworkError::from(error))),
		)?;
		Ok(rows.into_iter().next().map(|row| RetentionRules {
			document_snapshots: SnapshotPeriods {
				after_run_seconds: period(row.snapshot_after_run_seconds),
				max_age_seconds: period(row.snapshot_max_age_seconds),
			},
		}))
	}
	async fn store(
		&mut self,
		tenant: &str,
		rules: Option<&RetentionRules>,
		decider: &Decider,
	) -> Result<()> {
		let now = Utc::now();
		let tx = self.executor();
		let (sql, values) = Query::delete()
			.from_table(Alias::new(TenantRetentionRule::table_name()))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.build(PostgresQueryBuilder);
		native(
			tx.execute(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from),
		)?;
		let periods = rules.map(|rules| rules.document_snapshots);
		if let Some(periods) = periods {
			let row = TenantRetentionRule::build()
				.tenant(tenant.to_owned())
				.snapshot_after_run_seconds(seconds(periods.after_run_seconds))
				.snapshot_max_age_seconds(seconds(periods.max_age_seconds))
				.updated_at(now)
				.updated_by(decider.recorded())
				.finish();
			native(
				TenantRetentionRule::objects()
					.insert_with_executor(tx, &row)
					.await
					.map_err(|error| crate::Error::from(FrameworkError::from(error))),
			)?;
		}
		let change = TenantRetentionRuleHistory::build()
			.id(Uuid::now_v7())
			.tenant(tenant.to_owned())
			.snapshot_after_run_seconds(periods.map(|periods| seconds(periods.after_run_seconds)))
			.snapshot_max_age_seconds(periods.map(|periods| seconds(periods.max_age_seconds)))
			.changed_at(now)
			.changed_by(decider.recorded())
			.finish();
		native(
			TenantRetentionRuleHistory::objects()
				.insert_with_executor(tx, &change)
				.await
				.map_err(|error| crate::Error::from(FrameworkError::from(error))),
		)?;
		Ok(())
	}
	async fn history(
		&mut self,
		tenant: &str,
		offset: u64,
		limit: u32,
	) -> Result<Vec<RetentionChange>> {
		let rows = native(
			TenantRetentionRuleHistory::objects()
				.filter(TenantRetentionRuleHistory::field_tenant().eq(tenant.to_owned()))
				.order_by(&["-changed_at", "-id"])
				.offset(usize::try_from(offset).unwrap_or(usize::MAX))
				.limit(limit as usize)
				.all_with_executor(self.executor())
				.await
				.map_err(|error| crate::Error::from(FrameworkError::from(error))),
		)?;
		Ok(rows
			.into_iter()
			.map(|row| RetentionChange {
				rules: row
					.snapshot_after_run_seconds
					.zip(row.snapshot_max_age_seconds)
					.map(|(after_run, max_age)| RetentionRules {
						document_snapshots: SnapshotPeriods {
							after_run_seconds: period(after_run),
							max_age_seconds: period(max_age),
						},
					}),
				changed_at: row.changed_at,
				changed_by: row.changed_by,
			})
			.collect())
	}
	async fn finish(self: Box<Self>, committed: bool) -> Result<()> {
		match self.tx {
			Transaction::Subject(access) => {
				let result = if committed {
					Ok(())
				} else {
					Err(crate::Error::Conflict("retention change abandoned".into()))
				};
				match access.finish(result).await {
					Err(crate::Error::Conflict(_)) if !committed => Ok(()),
					other => native(other),
				}
			}
			Transaction::Administrator(tx) => native(
				async {
					if committed {
						tx.commit().await?;
					} else {
						tx.rollback().await?;
					}
					Ok(())
				}
				.await,
			),
		}
	}
}

#[async_trait]
impl RetentionRepository for TenantRetention<'_> {
	async fn begin(&self) -> Result<Box<dyn RetentionScope + '_>> {
		let (decider, tx) = match self.actor {
			Actor::Subject(identity) => (
				Decider::Subject {
					tenant: identity.tenant.clone(),
					subject: identity.subject.clone(),
				},
				Transaction::Subject(Box::new(native(
					NativeAccess::begin(self.store, identity).await,
				)?)),
			),
			Actor::DeploymentAdministrator => (
				Decider::DeploymentAdministrator {
					identity: self.administrator,
				},
				Transaction::Administrator(native(
					self.store
						.database()
						.begin()
						.await
						.map_err(crate::Error::from),
				)?),
			),
		};
		Ok(Box::new(Scope {
			decider,
			deployment: self.deployment,
			tx,
		}))
	}
}
