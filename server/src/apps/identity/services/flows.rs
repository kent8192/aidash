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
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::tenant_administration::NODE_ADMINISTER;
use aidash_domain::{ControlSource, RunControlAction};
use chrono::Utc;
use serde_json::json;
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
			if !aidash_application::flows::is_root(&mut graph, root).await? {
				return Err(Error::Invalid(
					"only a Root Task's flow is stopped; this Task was created under another"
						.into(),
				));
			}
			let root_runs = aidash_application::flows::FlowGraph::runs(&mut graph, &[root]).await?;
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
				let resource = access.resource("node", &node, json!({}));
				allowed = access.decide(&resource, NODE_ADMINISTER).await?;
			}
			if !allowed {
				return Err(Error::Forbidden);
			}
			let mut graph = NativeFlowGraph {
				tx: &mut *access.tx,
				tenant: tenant.clone(),
			};
			let flow = aidash_application::flows::closure(&mut graph, root).await?;
			let runs: Vec<Uuid> = flow.runs.iter().copied().collect();
			let open = graph.open_runs(&runs).await?;
			let tasks: Vec<Uuid> = flow.tasks.iter().copied().collect();
			let grants = graph.remote_grants(&tasks).await?;
			Ok((flow, open, grants))
		}
		.await;
		let (flow, open, grants) = access.finish(result).await?;
		let scope = TenancyScope::tenant(&tenant)?;
		let tasks: Vec<Uuid> = flow.tasks.iter().copied().collect();

		// Runs first: cancelling a Run also ends the Subnodes it created.
		let mut runs = vec![];
		for id in &flow.runs {
			if !open.contains(id) {
				runs.push(stopped(*id, StopOutcome::AlreadyStopped));
				continue;
			}
			match f.store.control(&scope, *id, RunControlAction::Cancel).await {
				Ok(_) => runs.push(stopped(*id, StopOutcome::Stopped)),
				Err(error) => runs.push(failed(*id, error)),
			}
		}

		// Work handed to another Deployment: the grant is revoked, so it
		// renews no more, and the receiver cancels its Run (decision 23). The
		// person stopping the flow need not be the grant's root person, so
		// this runs as the system, as a Node deletion's cancellation does.
		let mut remote = vec![];
		for (grant, settled) in grants {
			if settled {
				remote.push(stopped(grant, StopOutcome::AlreadyStopped));
				continue;
			}
			match crate::apps::identity::repositories::home_execution::cancel_flow_grant(
				f, &tenant, grant,
			)
			.await
			{
				Ok(()) => remote.push(stopped(grant, StopOutcome::Stopped)),
				Err(error) => remote.push(failed(grant, error)),
			}
		}

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
		let mut creators = std::collections::BTreeMap::<Uuid, Vec<Uuid>>::new();
		for (subnode, creator) in active {
			creators.entry(creator).or_default().push(subnode);
		}
		for (creator, ids) in creators {
			let ended: Result<()> = async {
				let mut tx = f.store.pool.begin(&scope).await?.into_executor().await?;
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

		// Duties pause; only their Requester resumes them.
		let duty_ids: Vec<Uuid> = flow.duties.iter().copied().collect();
		let duties = match f
			.store
			.pool
			.atomic(&scope, async |tx| {
				crate::apps::workspaces::repositories::duties::stop(
					tx,
					&tenant,
					crate::apps::workspaces::repositories::duties::Stop::Flow(&duty_ids),
					Utc::now(),
				)
				.await
			})
			.await
		{
			Ok(paused) => {
				let paused: std::collections::BTreeSet<Uuid> =
					paused.iter().map(|duty| duty.id).collect();
				duty_ids
					.iter()
					.map(|id| {
						stopped(
							*id,
							if paused.contains(id) {
								StopOutcome::Stopped
							} else {
								StopOutcome::AlreadyStopped
							},
						)
					})
					.collect()
			}
			Err(error) => duty_ids
				.iter()
				.map(|id| failed(*id, error.to_string()))
				.collect(),
		};

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
			tasks: flow.tasks.len(),
			runs,
			remote,
			subnodes,
			duties,
			nodes,
		})
	}
}
