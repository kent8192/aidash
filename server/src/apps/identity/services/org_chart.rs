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
use aidash_domain::nodes::{DeploymentId, NodeAddress, NodeName};
use aidash_domain::registry::EntityRef;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
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

/// The Node of each Thread.
async fn thread_nodes(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	threads: Vec<Uuid>,
) -> Result<BTreeMap<Uuid, String>> {
	if threads.is_empty() {
		return Ok(BTreeMap::new());
	}
	let rows: Vec<(Uuid, String)> = native::query_as(
		&Query::select()
			.columns([Alias::new("id"), Alias::new("node")])
			.from(Alias::new("threads"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").is_in(threads.into_iter().map(Expr::value)))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["id", "node"])
	.fetch_all(&mut *tx)
	.await?;
	Ok(rows.into_iter().collect())
}

/// The Node name of a stored Subnode: a Subnode records the address of its
/// Node (`aidash://<deployment>/<name>`, as `create_subnode` writes it), and
/// older rows the bare name. `None` for a Node of another Deployment, which
/// no Node of this chart holds.
fn subnode_node(deployment: &DeploymentId, stored: &str) -> Option<String> {
	match NodeAddress::parse(stored) {
		Ok(address) => (address.deployment == *deployment).then(|| address.node.into_string()),
		Err(_) => NodeName::parse(stored).ok().map(NodeName::into_string),
	}
}

/// The Tenant's active Subnodes, with their members and the person at the
/// root of each creating Run's chain.
async fn subnodes(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	deployment: &DeploymentId,
) -> Result<Vec<SubnodeRow>> {
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
		.filter_map(|record| {
			Some(SubnodeRow {
				id: record.id,
				node: subnode_node(deployment, &record.node)?,
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
		})
		.collect())
}

/// What a Duty's own page checks for the person (`DutyScope::sees_thread`
/// and `administers`): the Threads holding Duties that they may read, and
/// whether they hold `duty.administer`.
async fn duty_visibility(
	store: &crate::store::Store,
	identity: &crate::authorization::identity::SubjectIdentity,
	tenant: &str,
) -> Result<(BTreeSet<Uuid>, bool)> {
	let mut access = crate::authorization::access::Access::begin(store, identity).await?;
	let result = async {
		let threads: Vec<Uuid> = native::query_scalar(
			&Query::select()
				.distinct()
				.column(Alias::new("thread_id"))
				.from(Alias::new("duties"))
				.and_where(Expr::col("tenant").eq(tenant.to_owned()))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&mut **access.tx)
		.await?;
		let mut readable = BTreeSet::new();
		for thread in threads {
			let resource = match access.thread(thread).await {
				Ok(resource) => resource,
				Err(crate::Error::NotFound(_) | crate::Error::Forbidden) => continue,
				Err(error) => return Err(error),
			};
			if access.decide(&resource, "thread.read").await? {
				readable.insert(thread);
			}
		}
		let resource = access.resource("tenant", tenant, json!({}));
		let administers = access
			.decide(&resource, aidash_domain::duties::ADMINISTER)
			.await?;
		Ok((readable, administers))
	}
	.await;
	access.finish(result).await
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
		let (threads, duty_administrator) =
			duty_visibility(&self.runtime.store, &administrator.identity, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = administrator.lock(tx, false).await?;
				let mut viewer = Viewer {
					subject: administrator.identity.subject.clone(),
					threads,
					duty_administrator,
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
				let nodes = thread_nodes(
					tx,
					&tenant,
					duties.iter().map(|duty| duty.thread_id).collect(),
				)
				.await?;
				rows.duties = duties
					.into_iter()
					.filter_map(|duty| {
						Some(DutyRow {
							id: duty.id,
							node: nodes.get(&duty.thread_id)?.clone(),
							thread: duty.thread_id,
							successor: duty.successor.clone(),
							state: duty.status().to_owned(),
							title: duty.definition.title.clone(),
							requester: duty.requester.clone(),
							agent: duty.definition.agent.clone(),
							created_by_task: creating.get(&duty.id).copied(),
						})
					})
					.collect();
				rows.subnodes = subnodes(tx, &tenant, &self.runtime.config.deployment).await?;
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
