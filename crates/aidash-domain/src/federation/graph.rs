//! Scoped graph identity, bounds, and projection rules independent of transport.
use crate::{
	Artifact, Conversation, Error, Event, Result, RunControl, RunMetadata, Task, Workspace,
	policy::identifier, registry::Entry,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphOptions {
	pub scope_workspace: Option<Uuid>,
	#[serde(default = "direct_depth")]
	#[schemars(range(min = 1, max = 1))]
	pub depth: u8,
	pub mode: String,

	pub kinds: Vec<String>,

	pub relations: Vec<String>,
	#[schemars(range(max = 720))]
	pub hours: u16,
	#[schemars(range(min = 2, max = 80))]
	pub limit: u16,
	#[schemars(length(max = 4096))]
	pub cursor: Option<String>,
	pub target_tenant: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GraphViewer {
	Subject { tenant: String, subject: String },
	Operator { id: Uuid },
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GraphNode {
	pub id: String,
	pub node_id: String,
	pub kind: String,
	pub name: BTreeMap<String, String>,
	pub resource_id: Option<String>,
	pub version: Option<String>,
	pub workspace_id: Option<Uuid>,
	pub status: Option<String>,
	pub goal_body: Option<String>,
	pub at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GraphEdge {
	pub source: String,
	pub target: String,
	pub relation: String,
	pub layer: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GraphPage {
	pub node_id: String,
	pub generation: String,
	pub checked_at: DateTime<Utc>,
	pub nodes: Vec<GraphNode>,
	pub edges: Vec<GraphEdge>,
	pub activity: Vec<GraphActivity>,
	pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GraphActivity {
	pub kind: String,
	pub at: DateTime<Utc>,
	pub reference: String,
}
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct GraphPeer {
	pub node_id: String,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphCursor {
	pub kind: u8,
	pub offset: u64,
	pub generation: String,
	pub binding: String,
	pub window_end: i64,
	pub expires_at: i64,
}
pub const CANDIDATE_BATCH: u64 = 64;
pub const CANDIDATE_KINDS: u8 = 6;
pub fn direct_depth() -> u8 {
	1
}
impl GraphOptions {
	pub fn validate(&self) -> Result<()> {
		if !matches!(
			self.mode.as_str(),
			"mesh" | "collaboration" | "knowledge" | "execution" | "topology"
		) || self.depth != direct_depth()
			|| self.limit < 2
			|| self.limit > 80
			|| self.hours > 720
			|| self.kinds.len() > 12
			|| self.relations.len() > 16
			|| self
				.cursor
				.as_ref()
				.is_some_and(|cursor| cursor.len() > 4096)
			|| self.kinds.iter().any(|kind| {
				!matches!(
					kind.as_str(),
					"workspace"
						| "goal" | "task" | "run"
						| "artifact" | "conversation"
						| "agent" | "cluster"
						| "tool" | "model" | "skill"
				)
			}) || self.relations.iter().any(|relation| {
			!matches!(
				relation.as_str(),
				"contains"
					| "goal" | "depends"
					| "produces" | "executes"
					| "tool" | "model"
					| "skill" | "member"
					| "hosts" | "coordinates"
					| "participates"
			)
		}) {
			return Err(Error::Invalid("invalid graph bounds".into()));
		}
		if let Some(tenant) = &self.target_tenant {
			identifier(tenant)?;
		}
		Ok(())
	}
}
#[derive(Clone)]
pub enum Candidate {
	Registry(Entry),
	Workspace(Workspace),
	Task(Task),
	Run(RunMetadata),
	Artifact(Artifact),
	Conversation(Conversation),
}
pub fn resource_key(node: &str, kind: &str, id: impl ToString) -> String {
	json!(["resource", node, kind, id.to_string()]).to_string()
}
pub fn entity_key(node: &str, kind: &str, id: &str, version: &str) -> String {
	json!(["entity", node, kind, id, version]).to_string()
}
pub fn kind_allowed(kind: &str, options: &GraphOptions) -> bool {
	if !options.kinds.iter().any(|allowed| allowed == kind) {
		return false;
	}
	match options.mode.as_str() {
		"topology" => matches!(kind, "agent" | "cluster" | "tool"),
		"execution" => matches!(kind, "goal" | "task" | "run" | "agent" | "artifact"),
		"collaboration" => matches!(
			kind,
			"workspace" | "goal" | "conversation" | "task" | "agent" | "cluster" | "artifact"
		),
		"knowledge" => kind != "run",
		_ => true,
	}
}
pub fn relation_allowed(relation: &str, options: &GraphOptions) -> bool {
	options.relations.iter().any(|allowed| allowed == relation)
}
pub fn registry_ref(value: &serde_json::Value) -> Option<(&str, &str)> {
	let id = value.get("id")?.as_str()?;
	let version = value.get("version")?.as_str()?;
	if id.is_empty() || version.is_empty() {
		None
	} else {
		Some((id, version))
	}
}
pub fn build_edges(
	node: &str,
	records: &[Candidate],
	nodes: &[GraphNode],
	options: &GraphOptions,
) -> Vec<GraphEdge> {
	let visible: BTreeSet<&str> = nodes.iter().map(|item| item.id.as_str()).collect();
	let mut edges = BTreeMap::<(String, String, String), GraphEdge>::new();
	let mut add = |source: String, target: String, relation: &str, layer: &str| {
		if source != target
			&& visible.contains(source.as_str())
			&& visible.contains(target.as_str())
			&& options.relations.iter().any(|allowed| allowed == relation)
		{
			edges.insert(
				(source.clone(), relation.to_owned(), target.clone()),
				GraphEdge {
					source,
					target,
					relation: relation.into(),
					layer: layer.into(),
				},
			);
		}
	};
	for record in records {
		match record {
			Candidate::Workspace(workspace) => add(
				resource_key(node, "workspace", workspace.id),
				resource_key(node, "goal", workspace.id),
				"goal",
				"configuration",
			),
			Candidate::Task(task) => {
				let id = resource_key(node, "task", task.id);
				add(
					resource_key(node, "workspace", task.workspace_id),
					id.clone(),
					"contains",
					"configuration",
				);
				if let Some(parent) = task.parent_id {
					add(
						resource_key(node, "task", parent),
						id.clone(),
						"contains",
						"activity",
					);
				} else {
					add(
						resource_key(node, "goal", task.workspace_id),
						id.clone(),
						"contains",
						"activity",
					);
				}
				for dep in &task.dependencies {
					add(
						resource_key(node, "task", dep),
						id.clone(),
						"depends",
						"activity",
					);
				}
			}
			Candidate::Run(run) => {
				let id = resource_key(node, "run", run.id);
				add(
					entity_key(node, "agent", &run.agent_id, &run.agent_version),
					id.clone(),
					"executes",
					"activity",
				);
				add(
					id,
					resource_key(node, "task", run.task_id),
					"executes",
					"activity",
				);
			}
			Candidate::Artifact(artifact) => add(
				resource_key(node, "task", artifact.task_id),
				resource_key(node, "artifact", artifact.id),
				"produces",
				"activity",
			),
			Candidate::Conversation(conversation) => {
				let id = resource_key(node, "conversation", conversation.id);
				add(
					resource_key(node, "workspace", conversation.workspace_id),
					id.clone(),
					"contains",
					"configuration",
				);
				if matches!(conversation.target_kind.as_str(), "agent" | "cluster")
					&& let Some((target, version)) = conversation.target.rsplit_once('@')
				{
					add(
						entity_key(node, &conversation.target_kind, target, version),
						id,
						"participates",
						"activity",
					);
				}
			}
			Candidate::Registry(entry) => {
				let id = entity_key(node, &entry.kind, &entry.id, &entry.version);
				if entry.kind == "cluster"
					&& let Some((target, version)) = registry_ref(&entry.config["coordinator"])
				{
					add(
						id.clone(),
						entity_key(node, "agent", target, version),
						"coordinates",
						"configuration",
					);
				}
				if entry.kind == "agent" {
					for (kind, field, relation) in [
						("tool", "tools", "tool"),
						("model", "model", "model"),
						("skill", "skills", "skill"),
						("cluster", "cluster", "member"),
					] {
						let value = &entry.config[field];
						let refs = value
							.as_array()
							.map(|items| items.iter().collect::<Vec<_>>())
							.unwrap_or_else(|| vec![value]);
						for item in refs {
							if let Some((target, version)) = registry_ref(item) {
								add(
									id.clone(),
									entity_key(node, kind, target, version),
									relation,
									"configuration",
								);
							}
						}
					}
				}
			}
		}
	}
	edges.into_values().take(600).collect()
}
pub fn event_reference(event: &Event, node: &str) -> Option<String> {
	let id = |value: &serde_json::Value| value.as_str().and_then(|text| text.parse::<Uuid>().ok());
	let data = &event.data;
	if event.kind.starts_with("run.") {
		return id(&data["run_id"])
			.or_else(|| id(&data["id"]))
			.map(|id| resource_key(node, "run", id));
	}
	let workspace = event.workspace_id?;
	if event.kind.starts_with("task.") {
		return id(&data["task"]["id"])
			.or_else(|| id(&data["task_id"]))
			.or_else(|| id(&data["id"]))
			.map(|id| resource_key(node, "task", id));
	}
	if event.kind.starts_with("artifact.") {
		return id(&data["id"]).map(|id| resource_key(node, "artifact", id));
	}
	if event.kind.starts_with("conversation.") {
		return id(&data["id"]).map(|id| resource_key(node, "conversation", id));
	}
	if event.kind.starts_with("message.") || event.kind.starts_with("workspace.") {
		return Some(resource_key(node, "workspace", workspace));
	}
	None
}
impl Candidate {
	pub fn kind(&self) -> &str {
		match self {
			Self::Registry(entry) => &entry.kind,
			Self::Workspace(_) => "workspace",
			Self::Task(_) => "task",
			Self::Run(_) => "run",
			Self::Artifact(_) => "artifact",
			Self::Conversation(_) => "conversation",
		}
	}
	pub fn nodes(&self, node_id: &str, options: &GraphOptions) -> Vec<GraphNode> {
		let mut nodes = Vec::new();
		let name = |text: &str| BTreeMap::from([("en".to_owned(), text.to_owned())]);
		match self {
			Self::Registry(entry) => nodes.push(GraphNode {
				id: entity_key(node_id, &entry.kind, &entry.id, &entry.version),
				node_id: node_id.to_owned(),
				kind: entry.kind.clone(),
				name: entry.name.clone(),
				resource_id: Some(entry.id.clone()),
				version: Some(entry.version.clone()),
				workspace_id: None,
				status: None,
				goal_body: None,
				at: None,
			}),
			Self::Workspace(workspace) => {
				if kind_allowed("workspace", options) {
					nodes.push(GraphNode {
						id: resource_key(node_id, "workspace", workspace.id),
						node_id: node_id.to_owned(),
						kind: "workspace".into(),
						name: name(&workspace.title),
						resource_id: Some(workspace.id.to_string()),
						version: None,
						workspace_id: Some(workspace.id),
						status: None,
						goal_body: None,
						at: Some(workspace.created_at),
					});
				}
				if !workspace.goal.trim().is_empty() && kind_allowed("goal", options) {
					let label = workspace
						.goal
						.lines()
						.find(|line| !line.trim().is_empty())
						.unwrap_or("Goal");
					nodes.push(GraphNode {
						id: resource_key(node_id, "goal", workspace.id),
						node_id: node_id.to_owned(),
						kind: "goal".into(),
						name: name(&label.chars().take(100).collect::<String>()),
						resource_id: Some(workspace.id.to_string()),
						version: None,
						workspace_id: Some(workspace.id),
						status: None,
						goal_body: Some(workspace.goal.clone()),
						at: Some(workspace.created_at),
					});
				}
			}
			Self::Task(task) => nodes.push(GraphNode {
				id: resource_key(node_id, "task", task.id),
				node_id: node_id.to_owned(),
				kind: "task".into(),
				name: name(&task.title),
				resource_id: Some(task.id.to_string()),
				version: None,
				workspace_id: Some(task.workspace_id),
				status: Some(task.status.to_string()),
				goal_body: None,
				at: Some(task.created_at),
			}),
			Self::Run(run) => nodes.push(GraphNode {
				id: resource_key(node_id, "run", run.id),
				node_id: node_id.to_owned(),
				kind: "run".into(),
				name: name(&format!("Run {}", run.id.simple())),
				resource_id: Some(run.id.to_string()),
				version: None,
				workspace_id: Some(run.workspace_id),
				status: Some(if run.control == RunControl::Paused {
					"PAUSED".into()
				} else {
					run.phase().to_string()
				}),
				goal_body: None,
				at: Some(run.updated_at),
			}),
			Self::Artifact(artifact) => nodes.push(GraphNode {
				id: resource_key(node_id, "artifact", artifact.id),
				node_id: node_id.to_owned(),
				kind: "artifact".into(),
				name: name(&artifact.name),
				resource_id: Some(artifact.id.to_string()),
				version: None,
				workspace_id: Some(artifact.workspace_id),
				status: Some(artifact.kind.clone()),
				goal_body: None,
				at: Some(artifact.created_at),
			}),
			Self::Conversation(conversation) => nodes.push(GraphNode {
				id: resource_key(node_id, "conversation", conversation.id),
				node_id: node_id.to_owned(),
				kind: "conversation".into(),
				name: name("Conversation"),
				resource_id: Some(conversation.id.to_string()),
				version: None,
				workspace_id: Some(conversation.workspace_id),
				status: None,
				goal_body: None,
				at: Some(conversation.created_at),
			}),
		}
		nodes
	}
}
pub fn valid_remote_page(page: &GraphPage, node_id: &str, options: &GraphOptions) -> bool {
	if page.node_id != node_id
		|| page.nodes.len() > 80
		|| page.edges.len() > 600
		|| page.activity.len() > 80
		|| page
			.next_cursor
			.as_ref()
			.is_some_and(|cursor| cursor.len() > 4096)
		|| !page
			.generation
			.strip_prefix("sha256:")
			.is_some_and(|digest| {
				digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
			}) {
		return false;
	}
	let mut ids = BTreeSet::new();
	for node in &page.nodes {
		let registry = matches!(
			node.kind.as_str(),
			"agent" | "cluster" | "tool" | "model" | "skill"
		);
		if node.node_id != node_id
			|| !kind_allowed(&node.kind, options)
			|| node.resource_id.as_deref().is_none_or(str::is_empty)
			|| node.goal_body.is_some() && node.kind != "goal"
			|| registry != node.version.is_some()
		{
			return false;
		}
		let resource = node.resource_id.as_deref().unwrap_or_default();
		let expected = match &node.version {
			Some(version) => entity_key(node_id, &node.kind, resource, version),
			None => resource_key(node_id, &node.kind, resource),
		};
		if node.id != expected || !ids.insert(node.id.as_str()) {
			return false;
		}
	}
	page.activity
		.iter()
		.all(|item| ids.contains(item.reference.as_str()) && item.kind.len() <= 100)
		&& page.edges.iter().all(|edge| {
			ids.contains(edge.source.as_str())
				&& ids.contains(edge.target.as_str())
				&& options
					.relations
					.iter()
					.any(|relation| relation == &edge.relation)
				&& matches!(
					edge.layer.as_str(),
					"configuration" | "activity" | "federation"
				)
		})
}

#[cfg(test)]
mod tests;
