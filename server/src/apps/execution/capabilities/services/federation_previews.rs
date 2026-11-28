//! Hosting Node authority for Federation File and Live preview tickets.
use crate::{Error, Result, authorization::identity::Actor, federation::Federation};
use aidash_application::federation::inbound::Admitted;
use aidash_domain::federation::{
	execution::Description,
	preview::{File, InspectRequest, Inspection, Live, Target, Ticket, TicketRequest},
};
use aidash_domain::{federation::route::Sender, nodes::NodeAddress, tenancy::TenantName};
use http::Method;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::json;

/// The receiver's own bound on a Home's request about one of its admitted
/// Runs (#1013). The admission must still be open and the Run not ended. Once
/// the Run has started, its Home decides whether the grant still lives, and
/// has just asked as that Home: a started grant outlives its pinned lifetime
/// while its Task is open. A Run that never started is still bound by it.
async fn require_live_admission(
	access: &mut crate::authorization::access::Access,
	description: &Description,
	run: &aidash_domain::Run,
) -> Result<()> {
	crate::apps::execution::repositories::node_cancellations::require_admission_open(
		&mut **access.tx,
		run.id,
	)
	.await?;
	if !aidash_domain::federation::execution::admission::serves_home(
		run.phase(),
		run.control,
		run.step,
		description.expires_at,
		chrono::Utc::now(),
	) {
		return Err(Error::Forbidden);
	}
	Ok(())
}

/// The source Deployment has a durable admission binding, but no copy of H's
/// Run row. Resolve the exact requester and target from that binding, and the
/// grant it binds.
async fn source_target(
	f: &Federation,
	access: &mut crate::authorization::access::Access,
	run_id: uuid::Uuid,
) -> Result<(Sender, String, uuid::Uuid)> {
	let binding: Option<(uuid::Uuid, uuid::Uuid)> = crate::database::native::query_as(
		&Query::select()
			.columns(["grant_id", "task_id"].map(Alias::new))
			.from(Alias::new("authorization_remote_execution"))
			.and_where(Expr::col("admission_id").eq(Expr::value(run_id)))
			.and_where(Expr::col("tenant").eq(Expr::value(&access.identity.tenant)))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["grant_id", "task_id"])
	.fetch_optional(&mut **access.tx)
	.await?;
	let (grant_id, task_id) = binding.ok_or(Error::Forbidden)?;
	let mut grant_query = Query::select();
	grant_query
		.columns(["node_id", "root_subject", "thread_id"].map(Alias::new))
		.from(Alias::new("authorization_remote_grants"))
		.and_where(Expr::col("id").eq(Expr::value(grant_id)))
		.and_where(Expr::col("task_id").eq(Expr::value(task_id)))
		.and_where(Expr::col("tenant").eq(Expr::value(&access.identity.tenant)));
	grant_query.and_where(
		crate::apps::identity::repositories::remote_grants::live_condition(
			"authorization_remote_grants",
		),
	);
	let grant: Option<(String, String, uuid::Uuid)> =
		crate::database::native::query_as(&grant_query.to_string(PostgresQueryBuilder))
			.columns(&["node_id", "root_subject", "thread_id"])
			.fetch_optional(&mut **access.tx)
			.await?;
	let (target, requester, thread_id) = grant.ok_or(Error::Forbidden)?;
	let task = access.task_read(task_id).await?;
	if requester != access.identity.subject || task.thread_id != thread_id {
		return Err(Error::Forbidden);
	}
	let source = f
		.config
		.deployment
		.node(access.thread_node(thread_id).await?);
	NodeAddress::parse(&target)?;
	Ok((
		Sender {
			tenant: TenantName::new(access.identity.tenant.clone())?,
			node: Some(source.node),
			subject: Some(requester),
		},
		target,
		grant_id,
	))
}

/// The Viewer's own lease on a remote Run, then the grant's whole chain: the
/// Home describes the grant as it would for its receiver, which re-evaluates
/// every subject of the chain by the current policy. A started grant stays
/// live by its activation while its Run sleeps (#1013), so without this a
/// chain broken meanwhile would keep reaching the Run's Working Area. Both
/// transactions end before the Viewer's request goes to the receiver.
async fn viewer_target(
	f: &Federation,
	identity: &crate::authorization::identity::SubjectIdentity,
	run_id: uuid::Uuid,
) -> Result<(Sender, String)> {
	let mut access = crate::authorization::access::Access::begin(&f.store, identity).await?;
	let result = source_target(f, &mut access, run_id).await;
	let (sender, node, grant) = access.finish(result).await?;
	let (lease, _) = crate::authorization::remote::description_lease(f, &node, grant)
		.await
		.map_err(|error| match error {
			Error::RemoteExecutionRefused(_) | Error::Unauthorized => Error::Forbidden,
			error => error,
		})?;
	lease.finish(Ok(())).await?;
	Ok((sender, node))
}

/// V can display a remote citation only to the original requester while the
/// source Task is still readable. No local Run row exists on V.
pub(crate) async fn source_viewer(
	f: &Federation,
	access: &mut crate::authorization::access::Access,
	run_id: uuid::Uuid,
) -> Result<()> {
	source_target(f, access, run_id).await.map(|_| ())
}

pub(crate) async fn inspect(
	f: Federation,
	admitted: Admitted,
	request: InspectRequest,
) -> Result<Inspection> {
	let mut access = crate::apps::identity::services::peer::access(&f, &admitted).await?;
	let result = async {
		let registration = crate::apps::identity::repositories::node_registrations::get_locked(
			&mut **access.tx,
			admitted.tenant.as_str(),
			admitted.receiver.node.as_str(),
			&admitted.sender,
			true,
		)
		.await?
		.ok_or(Error::Forbidden)?;
		if !registration.enabled {
			return Err(Error::Forbidden);
		}
		let origin_registered = admitted.sender.deployment == f.config.deployment
			|| registration.dashboard_origin.is_some();
		let record = crate::apps::execution::repositories::previews::peer_run_admission(
			&mut access,
			request.run_id,
			&admitted.sender.to_string(),
		)
		.await?;
		let description: Description = serde_json::from_value(record.description)?;
		if record.node != admitted.receiver.node.as_str()
			|| record.tenant != admitted.tenant.as_str()
			|| record.credential_id != access.identity.credential_id
			|| description.source_subject != admitted.subject().ok_or(Error::Forbidden)?
			|| description.source_node != admitted.sender.to_string()
			|| description.target_node != admitted.receiver.to_string()
		{
			return Err(Error::Forbidden);
		}
		let persisted =
			crate::apps::execution::models::Run::read_in(&mut **access.tx, request.run_id, None)
				.await?
				.ok_or(Error::Forbidden)?;
		let run = persisted;
		if run.node != admitted.receiver.node
			|| run.home_node != description.source_node
			|| run.task_id != record.task_id
			|| run.thread_id != description.task.thread_id
		{
			return Err(Error::Forbidden);
		}
		require_live_admission(&mut access, &description, &run).await?;
		// A remote Run has an admission record rather than a local execution
		// grant. With no preview listener there is no area or viewing address to
		// disclose; V can show the explicit unavailable state.
		if f.config.preview.is_none() || !origin_registered {
			return Ok(Inspection {
				area_id: uuid::Uuid::nil(),
				files: Vec::new(),
				live_preview: None,
				preview_available: false,
			});
		}
		let area = super::sessions::for_run(&mut access, &run).await?;
		let files = super::service::files(&area)?
			.into_iter()
			.map(|entry| File {
				file_id: entry.file_id,
				path: entry.path,
				digest: entry.digest,
				size: entry.size,
				media_type: entry.media_type,
			})
			.collect();
		let live_preview = crate::apps::execution::repositories::live_previews::status(
			&f.store,
			&mut access,
			&run,
			f.config
				.preview
				.as_ref()
				.map(|preview| preview.domain.clone()),
		)
		.await?
		.map(|status| Live {
			id: status.id,
			state: status.state.as_str().to_owned(),
		});
		Ok(Inspection {
			area_id: area.id,
			files,
			live_preview,
			preview_available: f.config.preview.is_some() && origin_registered,
		})
	}
	.await;
	access.finish(result).await
}

/// H admits the sending Node and its mapped subject, verifies the immutable
/// delegated Run binding, and pins V's registered dashboard origin.
pub(crate) async fn issue(
	f: Federation,
	admitted: Admitted,
	request: TicketRequest,
) -> Result<Ticket> {
	let preview = f
		.config
		.preview
		.as_ref()
		.ok_or_else(|| Error::Conflict("PREVIEW_UNAVAILABLE".into()))?;
	let mut access = crate::apps::identity::services::peer::access(&f, &admitted).await?;
	let result = async {
		let registration = crate::apps::identity::repositories::node_registrations::get_locked(
			&mut **access.tx,
			admitted.tenant.as_str(),
			admitted.receiver.node.as_str(),
			&admitted.sender,
			true,
		)
		.await?
		.ok_or(Error::Forbidden)?;
		let pinned = if admitted.sender.deployment == f.config.deployment {
			f.config.dashboard_origin.clone()
		} else {
			registration.dashboard_origin.ok_or(Error::Forbidden)?
		};
		if !registration.enabled
			|| aidash_domain::preview::origin(&pinned)?
				!= aidash_domain::preview::origin(&request.frame_ancestor)?
		{
			return Err(Error::Forbidden);
		}
		let record = crate::apps::execution::repositories::previews::peer_run_admission(
			&mut access,
			request.run_id,
			&admitted.sender.to_string(),
		)
		.await?;
		let description: Description = serde_json::from_value(record.description)?;
		if record.node != admitted.receiver.node.as_str()
			|| record.tenant != admitted.tenant.as_str()
			|| record.credential_id != access.identity.credential_id
			|| description.source_subject != admitted.subject().ok_or(Error::Forbidden)?
			|| description.source_node != admitted.sender.to_string()
			|| description.target_node != admitted.receiver.to_string()
		{
			return Err(Error::Forbidden);
		}
		let persisted =
			crate::apps::execution::models::Run::read_in(&mut **access.tx, request.run_id, None)
				.await?
				.ok_or(Error::Forbidden)?;
		let run = persisted;
		if run.node != admitted.receiver.node
			|| run.home_node != description.source_node
			|| run.task_id != record.task_id
			|| run.thread_id != description.task.thread_id
		{
			return Err(Error::Forbidden);
		}
		require_live_admission(&mut access, &description, &run).await?;
		let area = super::sessions::for_run(&mut access, &run).await?;
		let peer = aidash_application::preview::Peer {
			sender: admitted.sender.to_string(),
			receiver: admitted.receiver.to_string(),
			subject: admitted.subject().ok_or(Error::Forbidden)?.to_owned(),
			run_id: run.id,
		};
		let issued = match request.target {
			Target::File { area_id, file_id } if area_id == area.id => {
				let mut scope = crate::apps::execution::repositories::previews::Scope {
					access: &mut access,
					store: &f.store,
					domain: Some(preview.domain.as_str()),
					public_port: preview.public_port,
					origin: &pinned,
					peer: Some(peer.clone()),
				};
				aidash_application::preview::issue_federated(&mut scope, area_id, file_id).await?
			}
			Target::Live { live_preview_id } => {
				let live = crate::apps::execution::repositories::live_previews::status(
					&f.store,
					&mut access,
					&run,
					Some(preview.domain.clone()),
				)
				.await?
				.ok_or_else(|| Error::NotFound("Live preview unavailable".into()))?;
				if live.id != live_preview_id {
					return Err(Error::Forbidden);
				}
				let mut scope = crate::apps::execution::repositories::previews::Scope {
					access: &mut access,
					store: &f.store,
					domain: Some(preview.domain.as_str()),
					public_port: preview.public_port,
					origin: &pinned,
					peer: Some(peer),
				};
				aidash_application::preview::issue_live(&mut scope, live_preview_id).await?
			}
			_ => return Err(Error::Forbidden),
		};
		Ok(Ticket {
			url: issued.url,
			expires_at: issued.expires_at,
			preview_domain: preview.domain.clone(),
		})
	}
	.await;
	access.finish(result).await
}

/// V checks its own viewer against the source Run's immutable requester before
/// asking H. The source authority transaction ends before peer I/O begins.
pub(crate) async fn request(
	f: Federation,
	actor: Actor,
	run_id: uuid::Uuid,
	target: Target,
	request_origin: Option<&str>,
) -> Result<Ticket> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let origin = aidash_domain::preview::origin(&f.config.dashboard_origin)?;
	if let Some(request_origin) = request_origin
		&& aidash_domain::preview::origin(request_origin)? != origin
	{
		return Err(Error::Forbidden);
	}
	let (sender, node) = viewer_target(&f, &identity, run_id).await?;
	let input = TicketRequest {
		run_id,
		target,
		frame_ancestor: origin,
	};
	f.request(
		&sender,
		&node,
		Method::POST,
		"/scoped/previews/tickets",
		Some(&json!(input)),
	)
	.await
}

/// V checks the same immutable source binding before a scoped metadata read.
pub(crate) async fn inspect_remote(
	f: Federation,
	actor: Actor,
	run_id: uuid::Uuid,
) -> Result<Inspection> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let (sender, node) = viewer_target(&f, &identity, run_id).await?;
	f.request(
		&sender,
		&node,
		Method::POST,
		"/scoped/previews/inspect",
		Some(&json!(InspectRequest { run_id })),
	)
	.await
}
