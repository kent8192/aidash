//! Subject authority is fixed by a durable participant reservation. Source
//! attempts survive process loss so a lost reply cannot complete revocation.
use super::{Manifest, Status, coordinator, gate};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	federation::Federation,
};
use futures_util::{TryStreamExt, stream};
use serde_json::{Value, json};
use uuid::Uuid;

fn request(manifest: &Manifest, origin: &Origin, node: &str) -> Result<Preflight> {
	aidash_application::transactions::authority::request(manifest, &origin.into(), node)
		.map(Into::into)
		.map_err(Into::into)
}

async fn checks(access: &mut Access, input: &Preflight, action: &str) -> Result<()> {
	aidash_application::transactions::authority::checks(
		&mut crate::bootstrap::transaction_authority_scope(access),
		&input.into(),
		action,
	)
	.await
	.map_err(Into::into)
}

pub(in crate::apps::federation::transactions) async fn preflight(
	f: &Federation,
	caller: &str,
	input: &Preflight,
) -> Result<()> {
	if !aidash_domain::transactions::authority::Preflight::from(input)
		.permits(caller, &f.config.node_id)
	{
		return Err(Error::Forbidden);
	}
	let mut access = mapped(f, input).await?;
	gate::read_in(&mut access.tx).await?;
	let result = async {
		checks(&mut access, input, "transaction.submit").await?;
		let binding = Binding {
			request: input.clone(),
			local: Origin::from(&access.identity),
			subjects: access.subjects.clone(),
		};
		bind(
			&mut access.tx,
			"atomic_preflights",
			input.id,
			&json!(binding),
		)
		.await
	}
	.await;
	access.finish(result).await
}
async fn source_checks(
	access: &mut Access,
	manifest: &Manifest,
	origin: &Origin,
	action: &str,
) -> Result<()> {
	aidash_application::transactions::authority::source_checks(
		&mut crate::bootstrap::transaction_authority_scope(access),
		manifest,
		&origin.into(),
		action,
	)
	.await
	.map_err(Into::into)
}
pub(in crate::apps::federation::transactions) async fn submit(
	f: &Federation,
	identity: &SubjectIdentity,
	manifest: &Manifest,
) -> Result<Status> {
	super::validate(manifest)?;
	if manifest.coordinator != f.config.node_id {
		return Err(Error::Invalid("submit to the named coordinator".into()));
	}
	let origin = Origin::from(identity);
	for node in &manifest.participants {
		request(manifest, &origin, &node.node_id)?;
	}
	let mut access = access(f, &origin).await?;
	let result = source_checks(&mut access, manifest, &origin, "transaction.submit").await;
	if let Err(error) = result {
		return access.finish(Err(error)).await;
	}
	match coordinator::status_with(&mut **access.tx, manifest.id).await {
		Ok(existing) => {
			let result = async {
				match_origin_with(&mut **access.tx, manifest.id, Some(&origin)).await?;
				if existing.digest != manifest.digest()? {
					return Err(Error::Conflict("transaction manifest is immutable".into()));
				}
				Ok(existing)
			}
			.await;
			return access.finish(result).await;
		}
		Err(Error::NotFound(_)) => {}
		Err(error) => return access.finish(Err(error)).await,
	}
	gate::read_in(&mut access.tx).await?;
	let subjects = access.subjects.clone();
	let mut access = access.into_native()?;
	let result = async {
		// Validate fresh admission before remote side effects. Keep the coordinator
		// uncommitted in this same authority transaction until every preflight
		// succeeds. Expiry during those RPCs then has a durable abort/recovery path.
		let stored = coordinator::submit_in(f, manifest, Some(&origin), access.tx.as_mut()).await?;
		let binding = Binding {
			request: request(manifest, &origin, &f.config.node_id)?,
			local: origin.clone(),
			subjects,
		};
		bind_native(
			access.tx.as_mut(),
			"atomic_preflights",
			manifest.id,
			&json!(binding),
		)
		.await?;
		// Participants make independent live policy decisions. Bound fan-out so
		// sixteen Nodes do not turn per-peer latency into a serial API timeout.
		stream::iter(
			manifest
				.participants
				.iter()
				.filter(|node| node.node_id != f.config.node_id)
				.map(Ok::<_, Error>),
		)
		.try_for_each_concurrent(8, |node| {
			let origin = &origin;
			async move {
				let input = request(manifest, origin, &node.node_id)?;
				let _: Value = coordinator::remote(
					f,
					&node.node_id,
					reqwest::Method::POST,
					"/transactions/preflight",
					Some(&input),
				)
				.await?;
				Ok(())
			}
		})
		.await?;
		super::fault::cut(manifest.id, "coordinator.submit.before").await?;
		Ok(stored)
	}
	.await;
	let stored = access.finish(result).await?;
	super::fault::cut(manifest.id, "coordinator.submit.after").await?;
	f.notify.notify_waiters();
	Ok(stored)
}

/// Issuing an attempt is serialized with source policy/credential revocation.
/// An unknown remote result remains pending until a durable reply or tombstone.
pub(super) async fn issue(f: &Federation, manifest: &Manifest, node: &str) -> Result<()> {
	let Some(origin) = binding::<Origin>(f, "atomic_subjects", manifest.id).await? else {
		return Ok(());
	};
	let mut access = access(f, &origin).await?;
	let result = async {
		source_checks(&mut access, manifest, &origin, "transaction.submit").await?;
		if node != f.config.node_id {
			trusted(&mut access, node).await?;
		}

		persistence::insert_attempt(&mut access, manifest.id, node).await?;
		Ok(())
	}
	.await;
	super::fault::cut(manifest.id, "authority.issue.before").await?;
	access.finish(result).await?;
	super::fault::cut(manifest.id, "authority.issue.after").await
}
pub(in crate::apps::federation::transactions) async fn ticket(
	f: &Federation,
	id: Uuid,
	node: &str,
) -> Result<Preflight> {
	let origin = binding::<Origin>(f, "atomic_subjects", id)
		.await?
		.ok_or(Error::Forbidden)?;
	let mut access = access(f, &origin).await?;
	let result = async {
		trusted(&mut access, node).await?;
		let state = coordinator::status_with(&mut **access.tx, id).await?;
		if state.decision.is_some() {
			return Err(Error::Forbidden);
		}
		let issued = persistence::attempt(&mut access, id, node).await?;
		if issued.is_none() {
			return Err(Error::Forbidden);
		}
		let manifest: Manifest = serde_json::from_value(state.manifest)?;
		source_checks(&mut access, &manifest, &origin, "transaction.submit").await?;
		request(&manifest, &origin, node)
	}
	.await;
	let proof = access.finish(result).await?;
	super::fault::cut(id, "authority.checked.after").await?;
	Ok(proof)
}
pub(super) async fn admission(
	f: &Federation,
	caller: &str,
	manifest: &Manifest,
) -> Result<Option<Access>> {
	let Some(bound) = binding::<Binding>(f, "atomic_preflights", manifest.id).await? else {
		if binding::<Origin>(f, "atomic_subjects", manifest.id)
			.await?
			.is_some()
		{
			return Err(Error::Forbidden);
		}
		return Ok(None);
	};
	if bound.request != request(manifest, &bound.request.origin, &f.config.node_id)?
		|| caller != bound.request.coordinator
	{
		return Err(Error::Forbidden);
	}
	let mut access = mapped(f, &bound.request).await?;
	let result = async {
		checks(&mut access, &bound.request, "transaction.submit").await?;
		if Origin::from(&access.identity) != bound.local || access.subjects != bound.subjects {
			return Err(Error::Forbidden);
		}
		if caller != f.config.node_id {
			let proof: Preflight = coordinator::remote(
				f,
				caller,
				reqwest::Method::GET,
				&format!("/transactions/{}/authority", manifest.id),
				None::<&()>,
			)
			.await?;
			if proof != bound.request {
				return Err(Error::Forbidden);
			}
		}
		Ok(())
	}
	.await;
	if let Err(error) = result {
		return access.finish(Err(error)).await;
	}
	Ok(Some(access))
}

// Immutable owner bindings hide both operator and other-subject transactions.
pub(in crate::apps::federation::transactions) async fn require_owner(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
) -> Result<Origin> {
	binding::<Origin>(f, "atomic_subjects", id)
		.await?
		.filter(|origin| origin.tenant == identity.tenant && origin.subject == identity.subject)
		.ok_or_else(|| Error::NotFound("transaction".into()))
}
pub(in crate::apps::federation::transactions) async fn manage(
	f: &Federation,
	identity: &SubjectIdentity,
	state: &Status,
	action: &str,
) -> Result<()> {
	let origin = require_owner(f, identity, state.id).await?;
	let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
	let mut access = access(f, &Origin::from(identity)).await?;
	// Live read checks are repeated by dashboard polling; mutation decisions stay audited.
	access.audit = action != "transaction.read";
	let result = async {
		source_checks(&mut access, &manifest, &origin, "transaction.read").await?;
		if action != "transaction.read" {
			let resource = access.resource(
				"transaction",
				state.id,
				json!({
					"coordinator": manifest.coordinator,
					"participants": manifest.participants.iter().map(|p| &p.node_id).collect::<Vec<_>>()
				}),
			);
			access.require(&resource, action).await?;
		}
		for node in &manifest.participants {
			if node.node_id != f.config.node_id {
				let input = request(&manifest, &origin, &node.node_id)?;
				let _: Value = coordinator::remote(
					f,
					&node.node_id,
					reqwest::Method::POST,
					"/transactions/access",
					Some(&input),
				)
				.await?;
			}
		}
		Ok(())
	}
	.await;
	if let Err(error) = result {
		return access.finish(Err(error)).await;
	}
	let mut access = access.into_native()?;
	let result = if action == "transaction.abort" {
		coordinator::abort_in(access.tx.as_mut(), state.id).await
	} else {
		Ok(())
	};
	access.finish(result).await?;
	if action == "transaction.abort" {
		super::fault::cut(state.id, "coordinator.abort.after").await?;
	}
	Ok(())
}
pub(in crate::apps::federation::transactions) async fn read_access(
	f: &Federation,
	caller: &str,
	input: &Preflight,
) -> Result<()> {
	if caller != input.coordinator {
		return Err(Error::Forbidden);
	}
	let bound = binding::<Binding>(f, "atomic_preflights", input.id)
		.await?
		.ok_or(Error::Forbidden)?;
	if bound.request != *input {
		return Err(Error::Forbidden);
	}
	let mut access = mapped(f, input).await?;
	access.audit = false;
	if Origin::from(&access.identity) != bound.local {
		return Err(Error::Forbidden);
	}
	let result = checks(&mut access, input, "transaction.read").await;
	access.finish(result).await
}

pub(crate) use crate::apps::federation::transactions::serializers::authority::{
	Binding, Origin, Preflight, Target,
};

impl From<&Origin> for aidash_domain::transactions::authority::Origin {
	fn from(origin: &Origin) -> Self {
		Self {
			credential_id: origin.credential_id,
			tenant: origin.tenant.clone(),
			subject: origin.subject.clone(),
		}
	}
}
impl From<aidash_domain::transactions::authority::Preflight> for Preflight {
	fn from(input: aidash_domain::transactions::authority::Preflight) -> Self {
		Self {
			id: input.id,
			coordinator: input.coordinator,
			digest: input.digest,
			origin: Origin {
				credential_id: input.origin.credential_id,
				tenant: input.origin.tenant,
				subject: input.origin.subject,
			},
			recipients: input.recipients,
			targets: input
				.targets
				.into_iter()
				.map(|target| Target {
					kind: target.kind,
					id: target.id,
					task_id: target.task_id,
				})
				.collect(),
		}
	}
}
impl From<&Preflight> for aidash_domain::transactions::authority::Preflight {
	fn from(input: &Preflight) -> Self {
		Self {
			id: input.id,
			coordinator: input.coordinator.clone(),
			digest: input.digest.clone(),
			origin: (&input.origin).into(),
			recipients: input.recipients.clone(),
			targets: input
				.targets
				.iter()
				.map(|target| aidash_domain::transactions::authority::Target {
					kind: target.kind.clone(),
					id: target.id,
					task_id: target.task_id,
				})
				.collect(),
		}
	}
}

use crate::apps::federation::transactions::repositories::authority::persistence::{
	self, access, binding, mapped, trusted,
};
pub(super) use persistence::{bind, bind_native, match_origin_native, scoped, settle};
pub(crate) use persistence::{control, pending};
pub(in crate::apps::federation::transactions) use persistence::{
	match_origin_with, pending_peer, run,
};
