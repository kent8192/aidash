//! Persistent Run-scoped Subnodes, membership, and Tenant limits.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "execution", table_name = "subnodes")]
#[derive(Serialize, Deserialize)]
pub struct SubnodeRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field]
	pub creator_run_id: Uuid,
	#[field]
	pub creator_task_id: Uuid,
	#[field(field_type = "text")]
	pub creator_agent_id: String,
	#[field(field_type = "text")]
	pub creator_agent_version: String,
	#[field(field_type = "text")]
	pub leader_agent_id: String,
	#[field(field_type = "text")]
	pub leader_agent_version: String,
	#[field]
	pub allowance: Json<Value>,
	#[field]
	pub workspace_id: Uuid,
	#[field(field_type = "text", null = true)]
	pub title: Option<String>,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub ended_at: Option<DateTime<Utc>>,
	#[field(field_type = "text", null = true)]
	pub ended_reason: Option<String>,
	#[field(field_type = "text")]
	pub creation_key: String,
	#[field(field_type = "text")]
	pub creation_fingerprint: String,
}

#[model(app_label = "execution", table_name = "subnode_members")]
#[derive(Serialize, Deserialize)]
pub struct SubnodeMemberRecord {
	#[field(primary_key = true)]
	pub subnode_id: Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub agent_version: String,
	#[field]
	pub added_by_run_id: Uuid,
	#[field]
	pub added_at: DateTime<Utc>,
}

#[model(app_label = "execution", table_name = "subnode_limits")]
#[derive(Serialize, Deserialize)]
pub struct SubnodeLimitRecord {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field]
	pub members_per_subnode: i32,
	#[field]
	pub subnodes_per_run: i32,
	#[field]
	pub remote_runs_may_create: bool,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub updated_by: String,
}

use crate::{Error, Result};
use aidash_domain::{
	registry::EntityRef,
	subnodes::{EndReason, Member, State, Subnode, SubnodeId, SubnodeLimits},
};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoValue, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};

fn framework(error: impl Into<reinhardt::core::exception::Error>) -> Error {
	Error::from(error.into())
}

impl SubnodeRecord {
	/// Caller holds the transaction and its workspace or creator lock.
	pub(crate) async fn domain(self, tx: &mut dyn TransactionExecutor) -> Result<Subnode> {
		let members = SubnodeMemberRecord::objects()
			.filter(SubnodeMemberRecord::field_tenant().eq(self.tenant.clone()))
			.filter(SubnodeMemberRecord::field_subnode_id().eq(self.id))
			.order_by(&["agent_id"])
			.all_with_executor(tx)
			.await
			.map_err(framework)?
			.into_iter()
			.map(|member| Member {
				agent_id: member.agent_id,
				agent_version: member.agent_version,
				added_by_run_id: member.added_by_run_id,
				added_at: member.added_at,
			})
			.collect();
		let state = match self.state.as_str() {
			"active" => State::Active,
			"dissolved" => State::Dissolved,
			_ => return Err(Error::Invalid("invalid persisted Subnode state".into())),
		};
		let ended_reason: Option<EndReason> = self
			.ended_reason
			.map(|value| serde_json::from_value(Value::String(value)))
			.transpose()?;
		Ok(Subnode {
			id: SubnodeId(self.id),
			tenant: self.tenant,
			node: self.node,
			creator_run_id: self.creator_run_id,
			creator_task_id: self.creator_task_id,
			creator_agent: EntityRef {
				id: self.creator_agent_id,
				version: self.creator_agent_version,
			},
			leader: EntityRef {
				id: self.leader_agent_id,
				version: self.leader_agent_version,
			},
			members,
			allowance: serde_json::from_value(self.allowance.0)?,
			workspace_id: self.workspace_id,
			title: self.title,
			state,
			created_at: self.created_at,
			ended_at: self.ended_at,
			ended_reason,
		})
	}

	pub(crate) async fn by_key(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		run: Uuid,
		key: &str,
	) -> Result<Option<Self>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant.to_owned()))
			.filter(Self::field_creator_run_id().eq(run))
			.filter(Self::field_creation_key().eq(key.to_owned()))
			.all_with_executor(tx)
			.await
			.map_err(framework)?
			.pop())
	}

	pub(crate) async fn active_count(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		run: Uuid,
	) -> Result<usize> {
		let count = Self::objects()
			.filter(Self::field_tenant().eq(tenant.to_owned()))
			.filter(Self::field_creator_run_id().eq(run))
			.filter(Self::field_state().eq("active"))
			.count_with_executor(tx)
			.await
			.map_err(framework)?;
		usize::try_from(count).map_err(|_| Error::Invalid("invalid Subnode count".into()))
	}

	pub(crate) async fn insert_active(
		tx: &mut dyn TransactionExecutor,
		value: &Subnode,
		key: &str,
		fingerprint: &str,
	) -> Result<()> {
		if value.state != State::Active || value.ended_at.is_some() || value.ended_reason.is_some()
		{
			return Err(Error::Invalid("new Subnode must be active".into()));
		}
		let row = Self {
			id: value.id.0,
			tenant: value.tenant.clone(),
			node: value.node.clone(),
			creator_run_id: value.creator_run_id,
			creator_task_id: value.creator_task_id,
			creator_agent_id: value.creator_agent.id.clone(),
			creator_agent_version: value.creator_agent.version.clone(),
			leader_agent_id: value.leader.id.clone(),
			leader_agent_version: value.leader.version.clone(),
			allowance: Json(serde_json::to_value(&value.allowance)?),
			workspace_id: value.workspace_id,
			title: value.title.clone(),
			state: "active".into(),
			created_at: value.created_at,
			ended_at: None,
			ended_reason: None,
			creation_key: key.into(),
			creation_fingerprint: fingerprint.into(),
		};
		Self::objects()
			.insert_with_executor(tx, &row)
			.await
			.map_err(framework)?;
		for member in &value.members {
			let row = SubnodeMemberRecord {
				subnode_id: value.id.0,
				tenant: value.tenant.clone(),
				agent_id: member.agent_id.clone(),
				agent_version: member.agent_version.clone(),
				added_by_run_id: member.added_by_run_id,
				added_at: member.added_at,
			};
			SubnodeMemberRecord::objects()
				.insert_with_executor(tx, &row)
				.await
				.map_err(framework)?;
		}
		Ok(())
	}
}

impl SubnodeLimitRecord {
	async fn lock_tenant(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		exclusive: bool,
	) -> Result<()> {
		let (sql, values) = Query::select()
			.column(Alias::new("name"))
			.from(Alias::new("tenants"))
			.and_where(Expr::col("name").eq(tenant.to_owned()))
			.lock(if exclusive {
				LockType::Update
			} else {
				LockType::Share
			})
			.build(PostgresQueryBuilder);
		tx.fetch_optional(&sql, convert_values(values))
			.await?
			.ok_or(Error::Forbidden)?;
		Ok(())
	}
	pub(crate) async fn get(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
	) -> Result<SubnodeLimits> {
		Self::lock_tenant(tx, tenant, false).await?;
		let Some(row) = Self::objects()
			.filter(Self::field_tenant().eq(tenant.to_owned()))
			.all_with_executor(tx)
			.await
			.map_err(framework)?
			.pop()
		else {
			return Ok(SubnodeLimits::default());
		};
		let limits = SubnodeLimits {
			members_per_subnode: u32::try_from(row.members_per_subnode)
				.map_err(|_| Error::Invalid("invalid Subnode limits".into()))?,
			subnodes_per_run: u32::try_from(row.subnodes_per_run)
				.map_err(|_| Error::Invalid("invalid Subnode limits".into()))?,
			remote_runs_may_create: row.remote_runs_may_create,
		};
		limits
			.validate()
			.map_err(|error| Error::Invalid(error.to_string()))?;
		Ok(limits)
	}
	pub(crate) async fn put(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		updated_by: &str,
		limits: SubnodeLimits,
	) -> Result<SubnodeLimits> {
		limits
			.validate()
			.map_err(|error| Error::Invalid(error.to_string()))?;
		Self::lock_tenant(tx, tenant, true).await?;
		let (sql, values) = Query::insert()
			.into_table(Alias::new("subnode_limits"))
			.columns(
				[
					"tenant",
					"members_per_subnode",
					"subnodes_per_run",
					"remote_runs_may_create",
					"updated_at",
					"updated_by",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(tenant),
				IntoValue::into_value(limits.members_per_subnode as i32),
				IntoValue::into_value(limits.subnodes_per_run as i32),
				IntoValue::into_value(limits.remote_runs_may_create),
				IntoValue::into_value(Utc::now()),
				IntoValue::into_value(updated_by),
			])
			.on_conflict(
				OnConflict::columns(["tenant"])
					.update_columns([
						"members_per_subnode",
						"subnodes_per_run",
						"remote_runs_may_create",
						"updated_at",
						"updated_by",
					])
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(limits)
	}
}

/// A completed model tool call. Effects and receipt commit in one transaction.
#[model(app_label = "execution", table_name = "subnode_operation_receipts")]
#[derive(Serialize, Deserialize)]
pub struct SubnodeOperationReceipt {
	#[field(primary_key = true)]
	pub run_id: Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub call_key: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub fingerprint: String,
	#[field]
	pub result: Json<Value>,
	#[field]
	pub created_at: DateTime<Utc>,
}
impl SubnodeOperationReceipt {
	pub(crate) async fn replay(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		run: Uuid,
		key: &str,
		fingerprint: &str,
	) -> Result<Option<Value>> {
		let row = Self::objects()
			.filter(Self::field_tenant().eq(tenant.to_owned()))
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_call_key().eq(key.to_owned()))
			.all_with_executor(tx)
			.await
			.map_err(framework)?
			.pop();
		match row {
			Some(row) if row.fingerprint != fingerprint => Err(Error::Conflict(
				"Subnode call key was reused with different input".into(),
			)),
			Some(row) => Ok(Some(row.result.0)),
			None => Ok(None),
		}
	}
	pub(crate) async fn save(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		run: Uuid,
		key: &str,
		fingerprint: &str,
		result: Value,
	) -> Result<()> {
		let row = Self {
			run_id: run,
			call_key: key.into(),
			tenant: tenant.into(),
			fingerprint: fingerprint.into(),
			result: Json(result),
			created_at: Utc::now(),
		};
		Self::objects()
			.insert_with_executor(tx, &row)
			.await
			.map_err(framework)?;
		Ok(())
	}
}
