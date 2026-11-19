//! The org chart (#1012, ADR 0143): for each Node a person may read, who
//! created it, its Duties, its active Subnodes and what they spent this month,
//! so people can see what each chain built, under whose authority and with
//! what spend. This module shapes what the person may see from rows the
//! server read; it reads nothing itself.
use aidash_domain::{nodes::agents::NodeCreator, registry::EntityRef};
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Who is looking, and what the Tenant's policy lets them see.
#[derive(Debug, Clone, Default)]
pub struct Viewer {
	pub subject: String,
	/// Nodes the person holds `node.read` on; only these are shown.
	pub reads: BTreeSet<String>,
	/// Nodes the person administers (`node.administer`): they see its spend.
	pub administers: BTreeSet<String>,
	/// `budget.manage`: every spend.
	pub budget_manager: bool,
}

/// One Node as stored.
#[derive(Debug, Clone)]
pub struct NodeRow {
	pub name: String,
	pub label: String,
	pub default: bool,
	pub creator: NodeCreator,
}

/// One Duty as stored, with the Node it works on.
#[derive(Debug, Clone)]
pub struct DutyRow {
	pub id: Uuid,
	pub node: String,
	pub title: String,
	pub requester: String,
	pub agent: EntityRef,
	pub state: String,
	/// The Task whose Run set it up, if an Agent did.
	pub created_by_task: Option<Uuid>,
}

/// One active Subnode, with the person at the root of its creator's chain.
#[derive(Debug, Clone)]
pub struct SubnodeRow {
	pub id: Uuid,
	pub node: String,
	pub title: Option<String>,
	pub creator_agent: EntityRef,
	pub creator_task: Uuid,
	pub root_subject: String,
	pub members: Vec<EntityRef>,
}

/// Spend in the ledger this month: reported cost and calls without one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Spend {
	pub nanocredits: i64,
	pub unpriced_calls: i64,
}

impl std::ops::AddAssign for Spend {
	fn add_assign(&mut self, other: Self) {
		self.nanocredits = self.nanocredits.saturating_add(other.nanocredits);
		self.unpriced_calls = self.unpriced_calls.saturating_add(other.unpriced_calls);
	}
}

/// This month's spend, by Node, Duty and root person.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
	pub nodes: BTreeMap<String, Spend>,
	pub duties: BTreeMap<Uuid, Spend>,
	pub people: BTreeMap<String, Spend>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ChartDuty {
	pub id: Uuid,
	pub title: String,
	pub requester: String,
	pub agent: EntityRef,
	/// `active`, `paused` or `stopped`.
	pub state: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub created_by_task: Option<Uuid>,
	/// The Root Task of the flow that set it up, which the stop action takes.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub flow: Option<Uuid>,
	/// Shown only to someone who may see this Duty's Budget.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub spend: Option<Spend>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ChartSubnode {
	pub id: Uuid,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	pub creator_agent: EntityRef,
	pub creator_task: Uuid,
	pub root_subject: String,
	pub members: Vec<EntityRef>,
	/// The Root Task of the flow whose Run created it.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub flow: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ChartNode {
	pub name: String,
	pub label: String,
	pub default: bool,
	pub created_by: NodeCreator,
	/// The Root Task of the flow whose Run created it, if an Agent's did.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub flow: Option<Uuid>,
	pub duties: Vec<ChartDuty>,
	pub subnodes: Vec<ChartSubnode>,
	/// Shown only to someone who may see this Node's Budget.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub spend: Option<Spend>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PersonSpend {
	pub subject: String,
	pub spend: Spend,
}

/// `GET /api/tenants/{tenant}/org-chart`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct OrgChart {
	/// The first instant of this UTC month, from which spend is counted.
	pub since: chrono::DateTime<chrono::Utc>,
	pub nodes: Vec<ChartNode>,
	/// Spend per root person: everyone's for a Budget manager, otherwise the
	/// person's own.
	pub people: Vec<PersonSpend>,
}

/// Everything the server read for one Tenant.
#[derive(Debug, Clone, Default)]
pub struct Rows {
	pub nodes: Vec<NodeRow>,
	pub duties: Vec<DutyRow>,
	pub subnodes: Vec<SubnodeRow>,
	pub ledger: Ledger,
	/// The Root Task of each creating Task.
	pub roots: BTreeMap<Uuid, Uuid>,
}

impl Rows {
	/// The creating Tasks whose Root Task the chart names.
	pub fn creating_tasks(&self) -> Vec<Uuid> {
		let mut tasks = BTreeSet::new();
		for node in &self.nodes {
			if let NodeCreator::Agent { task_id, .. } = &node.creator {
				tasks.insert(*task_id);
			}
		}
		tasks.extend(self.duties.iter().filter_map(|duty| duty.created_by_task));
		tasks.extend(self.subnodes.iter().map(|subnode| subnode.creator_task));
		tasks.into_iter().collect()
	}
}

/// What `viewer` may see of the Tenant. A Node shows, with its Duties and
/// active Subnodes, to whoever holds `node.read` on it. Spend shows only to
/// whoever may set that scope's Budget: `budget.manage`, a Node's
/// administrator, a Duty's Requester and a person for themself.
pub fn shape(viewer: &Viewer, rows: Rows, since: chrono::DateTime<chrono::Utc>) -> OrgChart {
	let root = |task: Option<Uuid>| task.map(|task| rows.roots.get(&task).copied().unwrap_or(task));
	let mut nodes = vec![];
	for node in &rows.nodes {
		if !viewer.reads.contains(&node.name) {
			continue;
		}
		let administers = viewer.administers.contains(&node.name);
		let mut chart = ChartNode {
			name: node.name.clone(),
			label: node.label.clone(),
			default: node.default,
			created_by: node.creator.clone(),
			flow: match &node.creator {
				NodeCreator::Agent { task_id, .. } => root(Some(*task_id)),
				_ => None,
			},
			duties: vec![],
			subnodes: vec![],
			spend: (viewer.budget_manager || administers).then(|| {
				rows.ledger
					.nodes
					.get(&node.name)
					.copied()
					.unwrap_or_default()
			}),
		};
		for duty in rows.duties.iter().filter(|duty| duty.node == node.name) {
			let own = duty.requester == viewer.subject;
			chart.duties.push(ChartDuty {
				id: duty.id,
				title: duty.title.clone(),
				requester: duty.requester.clone(),
				agent: duty.agent.clone(),
				state: duty.state.clone(),
				created_by_task: duty.created_by_task,
				flow: root(duty.created_by_task),
				spend: (viewer.budget_manager || administers || own).then(|| {
					rows.ledger
						.duties
						.get(&duty.id)
						.copied()
						.unwrap_or_default()
				}),
			});
		}
		for subnode in rows
			.subnodes
			.iter()
			.filter(|subnode| subnode.node == node.name)
		{
			chart.subnodes.push(ChartSubnode {
				id: subnode.id,
				title: subnode.title.clone(),
				creator_agent: subnode.creator_agent.clone(),
				creator_task: subnode.creator_task,
				root_subject: subnode.root_subject.clone(),
				members: subnode.members.clone(),
				flow: root(Some(subnode.creator_task)),
			});
		}
		nodes.push(chart);
	}
	let people = rows
		.ledger
		.people
		.iter()
		.filter(|(subject, _)| viewer.budget_manager || **subject == viewer.subject)
		.map(|(subject, spend)| PersonSpend {
			subject: subject.clone(),
			spend: *spend,
		})
		.collect();
	OrgChart {
		since,
		nodes,
		people,
	}
}

/// The first instant of the UTC month holding `now`, from which the chart
/// counts spend.
pub fn month_start(now: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
	use chrono::Datelike;
	now.date_naive()
		.with_day(1)
		.expect("the first day of a month")
		.and_hms_opt(0, 0, 0)
		.expect("midnight")
		.and_utc()
}

#[cfg(test)]
mod tests;
