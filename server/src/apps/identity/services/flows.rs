//! Stopping a flow (#1012, ADR 0143): one action stops everything a request's
//! chain made, including the work it handed to other Deployments. Only a
//! person's browser session stops one, and only someone who may control the
//! Root Task's Run (`run.control`) or administers the Root Task's Node
//! (`node.administer`, which Tenant Administrators hold).
use super::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	oidc::BrowserOrigin,
	repositories::flows::NativeFlowGraph,
	serializers::flows::{FlowNode, FlowStopped, StopOutcome, StoppedItem},
};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
};
use aidash_application::{
	flows::{self, Flow, FlowGraph},
	tenancy::TenancyScope,
};
use aidash_domain::identity::tenant_administration::NODE_ADMINISTER;
use aidash_domain::{ControlSource, RunControlAction};
use chrono::Utc;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

fn failed(id: Uuid, error: impl ToString) -> StoppedItem {
	StoppedItem {
		id,
		outcome: StopOutcome::Failed,
		error: Some(error.to_string()),
	}
}

fn stopped(id: Uuid, outcome: StopOutcome) -> StoppedItem {
	StoppedItem {
		id,
		outcome,
		error: None,
	}
}

/// Use the Home's authority for remote control: the grant's requester may
/// manage the Task and must hold `run.control` on its bound admission.
async fn may_control_remote_root(access: &mut Access, tenant: &str, root: Uuid) -> Result<bool> {
	let subject = access.identity.subject.clone();
	let admissions = NativeFlowGraph {
		tx: &mut *access.tx,
		tenant: tenant.to_owned(),
	}
	.root_admissions(root, &subject)
	.await?;
	if admissions.is_empty() {
		return Ok(false);
	}
	let task = crate::apps::workspaces::models::Task::read_many_in(&mut **access.tx, &[root])
		.await?
		.pop()
		.ok_or_else(|| Error::NotFound("task".into()))?;
	let resource = access.task_resource(&task).await?;
	if !access.decide(&resource, "task.delegate").await? {
		return Ok(false);
	}
	for admission in admissions {
		let run = access.resource("run", admission, resource.attributes.clone());
		if access.decide(&run, "run.control").await? {
			return Ok(true);
		}
	}
	Ok(false)
}

#[derive(Default)]
struct StoppedFlow {
	flow: Flow,
	runs: BTreeMap<Uuid, StoppedItem>,
	remote: BTreeMap<Uuid, StoppedItem>,
	duties: BTreeMap<Uuid, StoppedItem>,
}

/// Fence each producer, then walk again for work it committed before its fence.
async fn stop_producers(
	f: &crate::federation::Federation,
	scope: &TenancyScope,
	tenant: &str,
	root: Uuid,
) -> Result<StoppedFlow> {
	use crate::apps::workspaces::repositories::duties::{self, Stop};
	let mut result = StoppedFlow::default();
	loop {
		let (flow, open, grants) = f
			.store
			.pool
			.atomic(scope, async |tx| {
				let mut graph = NativeFlowGraph {
					tx,
					tenant: tenant.to_owned(),
				};
				let flow = flows::closure(&mut graph, root).await?;
				let runs: Vec<Uuid> = flow.runs.iter().copied().collect();
				let tasks: Vec<Uuid> = flow.tasks.iter().copied().collect();
				Ok((
					flow,
					graph.open_runs(&runs).await?,
					graph.remote_grants(&tasks).await?,
				))
			})
			.await?;
		let new_duties: Vec<Uuid> = flow
			.duties
			.iter()
			.copied()
			.filter(|id| !result.duties.contains_key(id))
			.collect();
		let new_runs: Vec<Uuid> = flow
			.runs
			.iter()
			.copied()
			.filter(|id| !result.runs.contains_key(id))
			.collect();
		let new_grants: Vec<_> = grants
			.into_iter()
			.filter(|(id, _, _)| !result.remote.contains_key(id))
			.collect();
		result.flow = flow;
		if new_duties.is_empty() && new_runs.is_empty() && new_grants.is_empty() {
			return Ok(result);
		}

		// Ticks and event-driven starts hold their Duty lock through commit.
		// Pausing waits for any in-flight start and prevents new scheduled work.
		if !new_duties.is_empty() {
			let paused = f
				.store
				.pool
				.atomic(scope, async |tx| {
					duties::stop(tx, tenant, Stop::Flow(&new_duties), Utc::now()).await
				})
				.await
				.map(|rows| {
					rows.into_iter()
						.map(|duty| duty.id)
						.collect::<BTreeSet<_>>()
				});
			for id in new_duties {
				let item = match &paused {
					Ok(ids) if ids.contains(&id) => stopped(id, StopOutcome::Stopped),
					Ok(_) => stopped(id, StopOutcome::AlreadyStopped),
					Err(error) => failed(id, error),
				};
				result.duties.insert(id, item);
			}
			// A start we waited for may have committed a Run or another Duty.
			continue;
		}

		// Cancelling a Run also ends the Subnodes it created.
		let open: BTreeSet<Uuid> = open.into_iter().collect();
		for id in new_runs {
			let item = if open.contains(&id) {
				match f.store.control(scope, id, RunControlAction::Cancel).await {
					Ok(_) => stopped(id, StopOutcome::Stopped),
					Err(error) => failed(id, error),
				}
			} else {
				stopped(id, StopOutcome::AlreadyStopped)
			};
			result.runs.insert(id, item);
		}

		// The action runs as the system, like a Node deletion's cancellation.
		// A grant whose Task already ended only needs revocation.
		for (grant, revoked, ended) in new_grants {
			use crate::apps::identity::repositories::home_execution;
			let outcome = match (revoked, ended) {
				(true, true) => Ok(StopOutcome::AlreadyStopped),
				(false, true) => home_execution::revoke_flow_grant(f, tenant, grant)
					.await
					.map(|()| StopOutcome::AlreadyStopped),
				(_, false) => home_execution::cancel_flow_grant(f, tenant, grant)
					.await
					.map(|()| StopOutcome::Stopped),
			};
			result.remote.insert(
				grant,
				match outcome {
					Ok(outcome) => stopped(grant, outcome),
					Err(error) => failed(grant, error),
				},
			);
		}
	}
}

impl TenantAdministrations {
	/// Stops the flow of `root`: cancels its open Runs and the work it handed
	/// to other Deployments, ends its Subnodes and pauses its Duties, each in
	/// its own transaction, and lists the Nodes it created, which stay. One
	/// item that fails does not keep the rest running.
	pub(crate) async fn stop_flow(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		root: Uuid,
	) -> Result<FlowStopped> {
		let f = &self.runtime;
		// A browser session only: no Agent stops a flow, even its own.
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let deployment = f.config.deployment.to_string();
		let mut access = Access::begin(&f.store, &administrator.identity).await?;
		let result = async {
			let mut graph = NativeFlowGraph {
				tx: &mut *access.tx,
				tenant: tenant.clone(),
			};
			let node = graph.task_node(root).await?;
			if !flows::is_root(&mut graph, root).await? {
				return Err(Error::Invalid(
					"only a Root Task's flow is stopped; this Task was created under another"
						.into(),
				));
			}
			let root_runs = graph.runs(&[root]).await?;
			let mut allowed = false;
			for run in root_runs {
				match crate::authorization::execution::management::readable(
					&mut access,
					run,
					&deployment,
				)
				.await
				{
					// A Run whose Thread an ended Subnode fences refuses
					// `run.control` with an error; the Node administrator may
					// still stop the flow.
					Ok((_, resource)) => match access.decide(&resource, "run.control").await {
						Ok(true) => {
							allowed = true;
							break;
						}
						Ok(false) | Err(Error::Forbidden) => {}
						Err(error) => return Err(error),
					},
					Err(Error::Forbidden | Error::NotFound(_)) => {}
					Err(error) => return Err(error),
				}
			}
			if !allowed {
				allowed = match may_control_remote_root(&mut access, &tenant, root).await {
					Ok(allowed) => allowed,
					Err(Error::Forbidden | Error::NotFound(_)) => false,
					Err(error) => return Err(error),
				};
			}
			if !allowed {
				let resource = access.resource("node", &node, json!({}));
				allowed = access.decide(&resource, NODE_ADMINISTER).await?;
			}
			if !allowed {
				return Err(Error::Forbidden);
			}
			Ok(())
		}
		.await;
		access.finish(result).await?;
		let scope = TenancyScope::tenant(&tenant)?;
		let stopped_flow = stop_producers(f, &scope, &tenant, root).await?;
		let tasks: Vec<Uuid> = stopped_flow.flow.tasks.iter().copied().collect();

		// Subnodes outlive a creator that completed; end the ones still active.
		let active = f
			.store
			.pool
			.atomic(&scope, async |tx| {
				NativeFlowGraph {
					tx,
					tenant: tenant.clone(),
				}
				.active_subnodes(&tasks)
				.await
			})
			.await?;
		let mut subnodes = vec![];
		let mut creators = BTreeMap::<Uuid, Vec<Uuid>>::new();
		for (subnode, creator) in active {
			creators.entry(creator).or_default().push(subnode);
		}
		for (creator, ids) in creators {
			let ended: Result<()> = async {
				let mut tx = f.store.pool.begin(&scope).await?.into_executor().await?;
				crate::apps::execution::repositories::subnodes::lock_for_end(tx.as_mut(), creator)
					.await?;
				f.store
					.subnode_control_in(
						tx.as_mut(),
						creator,
						RunControlAction::Cancel,
						ControlSource::Tenant,
						Some("dissolved"),
					)
					.await?;
				tx.commit().await?;
				Ok(())
			}
			.await;
			for id in ids {
				subnodes.push(match &ended {
					Ok(()) => stopped(id, StopOutcome::Stopped),
					Err(error) => failed(id, error),
				});
			}
		}

		let nodes = f
			.store
			.pool
			.atomic(&scope, async |tx| {
				crate::apps::identity::repositories::nodes::created_by_tasks(tx, &tenant, &tasks)
					.await
			})
			.await?
			.into_iter()
			.map(|node| FlowNode {
				name: node.name.into_string(),
				label: node.label,
			})
			.collect();
		f.notify.notify_waiters();
		Ok(FlowStopped {
			root_task: root,
			tasks: tasks.len(),
			runs: stopped_flow.runs.into_values().collect(),
			remote: stopped_flow.remote.into_values().collect(),
			subnodes,
			duties: stopped_flow.duties.into_values().collect(),
			nodes,
		})
	}
}
