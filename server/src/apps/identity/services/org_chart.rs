//! The org chart (#1012, ADR 0143): what each chain created, under whose
//! authority and with what spend. A person reads it from a browser session,
//! like Budgets, whose spend it shows; `aidash_application::org_chart`
//! decides what of it the person may see.
use super::tenant_administration::TenantAdministrations;
use crate::apps::execution::models::subnodes::{SubnodeMemberRecord, SubnodeRecord};
use crate::apps::identity::{oidc::BrowserOrigin, repositories::flows::NativeFlowGraph};
use crate::database::native;
use crate::{Result, authorization::identity::Actor};
use aidash_application::org_chart::{
	self as chart, DutyRow, Ledger, NodeRow, OrgChart, Rows, Spend, SubnodeRow, Viewer,
};
use aidash_domain::identity::tenant_administration::{BUDGET_MANAGE, NODE_ADMINISTER, NODE_READ};
use aidash_domain::registry::EntityRef;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

fn framework(error: impl Into<reinhardt::core::exception::Error>) -> crate::Error {
	crate::Error::from(error.into())
}

/// This month's spend in the ledger, by Node, Duty and root person.
async fn ledger(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	since: DateTime<Utc>,
) -> Result<Ledger> {
	let mut ledger = Ledger::default();
	let rows = native::query(
		&Query::select()
			.columns(
				[
					"node",
					"root_subject",
					"duty_id",
					"spent_nanocredits",
					"unpriced_calls",
				]
				.map(Alias::new),
			)
			.from(Alias::new("budget_spend"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("day").gte(Expr::value(since)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	for row in rows {
		let spend = Spend {
			nanocredits: row.try_get("spent_nanocredits")?,
			unpriced_calls: row.try_get("unpriced_calls")?,
		};
		let node: String = row.try_get("node")?;
		let person: String = row.try_get("root_subject")?;
		let duty: Uuid = row.try_get("duty_id")?;
		*ledger.nodes.entry(node).or_default() += spend;
		if !person.is_empty() {
			*ledger.people.entry(person).or_default() += spend;
		}
		if !duty.is_nil() {
			*ledger.duties.entry(duty).or_default() += spend;
		}
	}
	Ok(ledger)
}

/// The Node of each Workspace.
async fn workspace_nodes(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	workspaces: Vec<Uuid>,
) -> Result<BTreeMap<Uuid, String>> {
	if workspaces.is_empty() {
		return Ok(BTreeMap::new());
	}
	let rows: Vec<(Uuid, String)> = native::query_as(
		&Query::select()
			.columns([Alias::new("id"), Alias::new("node")])
			.from(Alias::new("workspaces"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").is_in(workspaces.into_iter().map(Expr::value)))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["id", "node"])
	.fetch_all(&mut *tx)
	.await?;
	Ok(rows.into_iter().collect())
}

/// The Tenant's active Subnodes, with their members and the person at the
/// root of each creating Run's chain.
async fn subnodes(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Vec<SubnodeRow>> {
	let records = SubnodeRecord::objects()
		.filter(SubnodeRecord::field_tenant().eq(tenant.to_owned()))
		.filter(SubnodeRecord::field_state().eq("active".to_owned()))
		.order_by(&["created_at", "id"])
		.all_with_executor(&mut *tx)
		.await
		.map_err(framework)?;
	if records.is_empty() {
		return Ok(vec![]);
	}
	let mut members = BTreeMap::<Uuid, Vec<EntityRef>>::new();
	for member in SubnodeMemberRecord::objects()
		.filter(SubnodeMemberRecord::field_tenant().eq(tenant.to_owned()))
		.filter(
			SubnodeMemberRecord::field_subnode_id()
				.is_in(records.iter().map(|record| record.id).collect::<Vec<_>>()),
		)
		.order_by(&["agent_id"])
		.all_with_executor(&mut *tx)
		.await
		.map_err(framework)?
	{
		members
			.entry(member.subnode_id)
			.or_default()
			.push(EntityRef {
				id: member.agent_id,
				version: member.agent_version,
			});
	}
	let roots: BTreeMap<Uuid, String> = native::query_as::<(Uuid, String)>(
		&Query::select()
			.columns([Alias::new("run_id"), Alias::new("root_subject")])
			.from(Alias::new("authorization_execution"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(
				Expr::col("run_id").is_in(
					records
						.iter()
						.map(|record| Expr::value(record.creator_run_id)),
				),
			)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["run_id", "root_subject"])
	.fetch_all(&mut *tx)
	.await?
	.into_iter()
	.collect();
	Ok(records
		.into_iter()
		.map(|record| SubnodeRow {
			id: record.id,
			node: record.node,
			title: record.title,
			creator_agent: EntityRef {
				id: record.creator_agent_id,
				version: record.creator_agent_version,
			},
			creator_task: record.creator_task_id,
			root_subject: roots
				.get(&record.creator_run_id)
				.cloned()
				.unwrap_or_default(),
			members: members.remove(&record.id).unwrap_or_default(),
		})
		.collect())
}

impl TenantAdministrations {
	/// `GET /api/tenants/{tenant}/org-chart`.
	pub(crate) async fn org_chart(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<OrgChart> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let since = chart::month_start(Utc::now());
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = administrator.lock(tx, false).await?;
				let mut viewer = Viewer {
					subject: administrator.identity.subject.clone(),
					..Viewer::default()
				};
				let live =
					crate::apps::identity::repositories::nodes::list_live(tx, &tenant).await?;
				for node in &live {
					let attributes = json!({"label": node.label, "default": node.default});
					let name = node.name.as_str();
					if !administrator
						.allowed(tx, &snapshot, NODE_READ, name, attributes.clone())
						.await?
					{
						continue;
					}
					viewer.reads.insert(name.to_owned());
					if administrator
						.allowed(tx, &snapshot, NODE_ADMINISTER, name, attributes)
						.await?
					{
						viewer.administers.insert(name.to_owned());
					}
				}
				viewer.duty_administrator = administrator
					.allowed(
						tx,
						&snapshot,
						aidash_domain::duties::ADMINISTER,
						&tenant,
						json!({}),
					)
					.await?;
				viewer.budget_manager = administrator
					.allowed(tx, &snapshot, BUDGET_MANAGE, &tenant, json!({}))
					.await?;
				let mut creators =
					crate::apps::identity::repositories::nodes::creators(tx, &tenant, None).await?;
				let mut rows = Rows {
					nodes: live
						.into_iter()
						.filter_map(|node| {
							let creator = creators.remove(node.name.as_str())?;
							Some(NodeRow {
								name: node.name.into_string(),
								label: node.label,
								default: node.default,
								creator,
							})
						})
						.collect(),
					..Rows::default()
				};
				let duties = crate::apps::workspaces::repositories::duties::list(
					tx,
					&tenant,
					crate::apps::workspaces::repositories::duties::Filter::Tenant,
				)
				.await?;
				let creating =
					crate::apps::workspaces::repositories::duties::creating_tasks(tx, &tenant)
						.await?;
				let nodes = workspace_nodes(
					tx,
					&tenant,
					duties.iter().map(|duty| duty.workspace_id).collect(),
				)
				.await?;
				rows.duties = duties
					.into_iter()
					.filter_map(|duty| {
						Some(DutyRow {
							id: duty.id,
							node: nodes.get(&duty.workspace_id)?.clone(),
							state: duty.status().to_owned(),
							title: duty.definition.title.clone(),
							requester: duty.requester.clone(),
							agent: duty.definition.agent.clone(),
							created_by_task: creating.get(&duty.id).copied(),
						})
					})
					.collect();
				rows.subnodes = subnodes(tx, &tenant).await?;
				rows.ledger = ledger(tx, &tenant, since).await?;
				let tasks = rows.creating_tasks();
				rows.roots = aidash_application::flows::roots(
					&mut NativeFlowGraph {
						tx,
						tenant: tenant.clone(),
					},
					&tasks,
				)
				.await?;
				Ok(chart::shape(&viewer, rows, since))
			})
			.await;
		self.finish(&administrator, result).await
	}
}
