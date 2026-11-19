//! Flows (#1012, ADR 0143): one Root Task, a request, and everything created
//! under it. Its Task tree, the Tasks its Runs created or delegated, the
//! Duties its Runs set up and the Tasks those Duties started all belong to it,
//! and so do the Subnodes and Nodes those Runs created. A person sees what a
//! flow made in the org chart and stops all of it with one action.
use crate::Result;
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// The links between a Tenant's Tasks, Runs and Duties that make a flow.
#[async_trait]
pub trait FlowGraph: Send {
	/// Tasks whose parent Task is one of `tasks`.
	async fn children(&mut self, tasks: &[Uuid]) -> Result<Vec<Uuid>>;
	/// The Runs of `tasks`.
	async fn runs(&mut self, tasks: &[Uuid]) -> Result<Vec<Uuid>>;
	/// Tasks that Runs of `runs` created or delegated.
	async fn created_by_runs(&mut self, runs: &[Uuid]) -> Result<Vec<Uuid>>;
	/// Duties a Run of `tasks`, or one of `runs`, set up.
	async fn duties(&mut self, tasks: &[Uuid], runs: &[Uuid]) -> Result<Vec<Uuid>>;
	/// Tasks the Duties started.
	async fn duty_tasks(&mut self, duties: &[Uuid]) -> Result<Vec<Uuid>>;
	/// What each of `tasks` was created under: its parent Task, else the Task
	/// of the Run that created it, else the Task whose Run set up the Duty that
	/// started it. A Task with none of them is a Root Task.
	async fn parents(&mut self, tasks: &[Uuid]) -> Result<BTreeMap<Uuid, Uuid>>;
}

/// Everything a flow made, found from its Root Task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flow {
	pub root: Uuid,
	pub tasks: BTreeSet<Uuid>,
	pub runs: BTreeSet<Uuid>,
	pub duties: BTreeSet<Uuid>,
}

fn fresh(found: Vec<Uuid>, known: &BTreeSet<Uuid>) -> Vec<Uuid> {
	found
		.into_iter()
		.filter(|id| !known.contains(id))
		.collect::<BTreeSet<_>>()
		.into_iter()
		.collect()
}

/// Walks the flow from `root` until nothing new turns up. Each pass reads one
/// layer for the whole frontier, and a link back to something already found
/// adds nothing, so a cycle ends the walk.
pub async fn closure(graph: &mut dyn FlowGraph, root: Uuid) -> Result<Flow> {
	let mut flow = Flow {
		root,
		tasks: BTreeSet::from([root]),
		..Flow::default()
	};
	let mut frontier = vec![root];
	while !frontier.is_empty() {
		let runs = fresh(graph.runs(&frontier).await?, &flow.runs);
		flow.runs.extend(runs.iter().copied());
		let mut next = graph.children(&frontier).await?;
		next.extend(graph.created_by_runs(&runs).await?);
		let duties = fresh(graph.duties(&frontier, &runs).await?, &flow.duties);
		flow.duties.extend(duties.iter().copied());
		next.extend(graph.duty_tasks(&duties).await?);
		frontier = fresh(next, &flow.tasks);
		flow.tasks.extend(frontier.iter().copied());
	}
	Ok(flow)
}

/// The Root Task of each of `tasks`: the top of the links
/// [`FlowGraph::parents`] reads. A Task a cycle leads back to is its own root
/// where the walk meets it again.
pub async fn roots(graph: &mut dyn FlowGraph, tasks: &[Uuid]) -> Result<BTreeMap<Uuid, Uuid>> {
	// The current top of each walk, and every Task each walk passed.
	let mut top: BTreeMap<Uuid, Uuid> = tasks.iter().map(|task| (*task, *task)).collect();
	let mut passed: BTreeMap<Uuid, BTreeSet<Uuid>> = tasks
		.iter()
		.map(|task| (*task, BTreeSet::from([*task])))
		.collect();
	let mut open: BTreeSet<Uuid> = tasks.iter().copied().collect();
	while !open.is_empty() {
		let asked: BTreeSet<Uuid> = open.iter().map(|task| top[task]).collect();
		let parents = graph
			.parents(&asked.into_iter().collect::<Vec<_>>())
			.await?;
		let mut still = BTreeSet::new();
		for task in open {
			let current = top[&task];
			let Some(parent) = parents.get(&current).copied() else {
				continue;
			};
			let seen = passed.get_mut(&task).expect("walk");
			if !seen.insert(parent) {
				continue;
			}
			top.insert(task, parent);
			still.insert(task);
		}
		open = still;
	}
	Ok(top)
}

/// Whether `task` is a Root Task: nothing created it under another Task.
pub async fn is_root(graph: &mut dyn FlowGraph, task: Uuid) -> Result<bool> {
	Ok(!graph.parents(&[task]).await?.contains_key(&task))
}

#[cfg(test)]
mod tests;
