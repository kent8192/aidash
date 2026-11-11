use crate::apps::identity::models::{
	AdministrationActor, AdministrationEvent, AuthorizationCatalog, database_time,
};
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::repositories::catalog_approvals::TenantCatalog;
use crate::apps::identity::repositories::catalog_requests as requests;
use crate::apps::identity::repositories::catalog_review::ReviewScope;
use crate::apps::identity::services::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	Authorization, Snapshot,
	catalog::Binding,
	identity::{Credential, IssuedCredential},
	policy::{Decision, Evaluation},
};
use crate::authorization::{access::Access, identity::Actor, identity::SubjectIdentity};
use crate::{Error, Result, federation::Federation};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::Model;
use reinhardt::injectable;

use serde_json::Value;

use uuid::Uuid;

fn control_service(f: Federation) -> Authorization {
	Authorization {
		pool: f.store.control_pool,
	}
}

use crate::apps::identity::serializers::policies::{
	AuthorizationPage, AuthorizationUpdate, CatalogApprovalInput, CatalogApprovalRequestPage,
	CatalogApprovalRequestView, CatalogImpactQuery, CatalogInput, CatalogMailPreference,
	CatalogPendingCount, CatalogRequestDecline, CatalogRequestInput, CatalogRequestNextAction,
	CatalogRequestPage, CatalogRequestReview, CatalogReviewQuery, CatalogReviewResult,
	CredentialInput, CredentialPage, CredentialRevocation, PolicyReplacement,
};
use aidash_application::authorization::catalog::review::{
	ApprovalReview, ApprovalReviewSubmission, ReviewItem,
};
use aidash_application::ports::catalog::CatalogMutation;
use aidash_domain::identity::catalog_request::{
	CATALOG_REQUEST_CREATE, CATALOG_REQUEST_DECLINE, CATALOG_REQUEST_FULFIL,
	CATALOG_REQUEST_WITHDRAW, CatalogApprovalRequest as ApprovalRequest,
	CatalogApprovalRequestState as RequestState, NewCatalogApprovalRequest,
};

/// A Tenant's policy, credentials and Catalog Approvals. A Deployment
/// Administrator manages any Tenant's; a Tenant's own subjects act through a
/// Mapping under its policy actions (#356, #357, #358).
/// Evaluation bodies are simulations of authority, never execution credentials.
#[derive(Clone)]
pub struct PolicyAdministration {
	runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PolicyAdministration {
	PolicyAdministration { runtime }
}

/// A Tenant route acts only for a subject of the Tenant it names.
fn tenant_subject(actor: Actor, tenant: &str) -> Result<SubjectIdentity> {
	match actor {
		Actor::Subject(identity) if identity.tenant == tenant => Ok(identity),
		_ => Err(Error::Forbidden),
	}
}

/// Parse `kind/id@version` or `id@version` from the review query.
fn review_entry(value: &str) -> Result<crate::registry::EntityRef> {
	let value = value.rsplit_once('/').map_or(value, |(_, rest)| rest);
	let (id, version) = value
		.rsplit_once('@')
		.filter(|(id, version)| !id.is_empty() && !version.is_empty())
		.ok_or_else(|| Error::Invalid("an entry is id@version".into()))?;
	Ok(crate::registry::EntityRef {
		id: id.into(),
		version: version.into(),
	})
}

fn request_audit_actor(identity: &SubjectIdentity) -> AdministrationActor {
	AdministrationActor::Subject {
		subject: identity.subject.clone(),
	}
}

async fn record_request_event(
	access: &mut Access,
	actor: &AdministrationActor,
	action: &str,
	tenant: &str,
	subject: &str,
) -> Result<()> {
	actor
		.record(
			&mut *access.tx,
			AdministrationEvent {
				action,
				identity: None,
				registration: None,
				mapping: None,
				tenant: Some(tenant),
				subject: Some(subject),
				invitation: None,
			},
		)
		.await
}

async fn request_review_for(
	access: &mut Access,
	store: &crate::store::Store,
	deployment: &aidash_domain::nodes::DeploymentId,
	node: &aidash_domain::nodes::NodeName,
	root: &crate::registry::EntityRef,
) -> Result<ApprovalReview> {
	access.set_node(node.clone());
	let mut scope = ReviewScope {
		access,
		include_node_revisions: false,
		node: deployment.node(node.clone()),
		validation: crate::bootstrap::registry_validation_for(store),
	};
	Ok(aidash_application::authorization::catalog::review::review(&mut scope, root).await?)
}

/// Record stopping carry as a new revision without changing approval or lineage.
async fn stop_carrying_revision(
	access: &mut Access,
	deployment: &aidash_domain::nodes::DeploymentId,
	node: &aidash_domain::nodes::NodeName,
	binding: &Binding,
	actor: &str,
) -> Result<Binding> {
	let mut scope = TenantCatalog {
		access,
		node: deployment.node(node.clone()),
		deployment_scope_orchestrated: false,
	};
	let reference = crate::registry::EntityRef {
		id: binding.entry_id.clone(),
		version: binding.entry_version.clone(),
	};
	let updated = scope
		.compare_and_set(
			&binding.tenant,
			node,
			&reference,
			binding.revision,
			binding.enabled,
			aidash_domain::identity::catalog::RestrictionChange::Preserve,
		)
		.await?
		.ok_or_else(|| Error::Conflict("catalog revision changed".into()))?;
	scope.history(&updated, actor, None).await?;
	scope.stop_carrying(&updated).await?;
	Ok(updated)
}

/// Apply a selected approval on every Node in its scope under one policy lease.
async fn approve_scoped(
	access: &mut Access,
	deployment: &aidash_domain::nodes::DeploymentId,
	source: &aidash_domain::nodes::NodeName,
	approvals: &[aidash_application::authorization::catalog::approvals::Approval],
	satisfied: &std::collections::BTreeSet<(String, String)>,
	satisfied_other_nodes: &std::collections::BTreeSet<(String, String)>,
	extra_subjects: &std::collections::BTreeSet<String>,
) -> Result<Vec<Binding>> {
	use aidash_domain::identity::catalog::ApprovalScope;
	let tenant = access.identity.tenant.clone();
	let all_nodes = approvals
		.iter()
		.any(|item| item.enabled && item.scope == ApprovalScope::Deployment);
	let nodes = if all_nodes {
		crate::apps::identity::repositories::nodes::lock_tenant(&mut **access.tx, &tenant).await?;
		crate::apps::identity::repositories::nodes::list_live(&mut **access.tx, &tenant)
			.await?
			.into_iter()
			.map(|node| node.name)
			.collect::<Vec<_>>()
	} else {
		vec![]
	};
	access.set_node(source.clone());
	let source_address = deployment.node(source.clone());
	let source_bindings =
		aidash_application::authorization::catalog::approvals::approve_with_satisfied(
			&mut TenantCatalog {
				access,
				node: source_address.clone(),
				deployment_scope_orchestrated: all_nodes,
			},
			approvals,
			satisfied,
			extra_subjects,
		)
		.await?;
	for node in nodes.into_iter().filter(|node| node != source) {
		let address = deployment.node(node.clone());
		let old_prefix = format!("{source_address}/agents/");
		let new_prefix = format!("{address}/agents/");
		let translate = |subject: &String| {
			subject
				.strip_prefix(&old_prefix)
				.map_or_else(|| subject.clone(), |rest| format!("{new_prefix}{rest}"))
		};
		let mut selected = Vec::new();
		for item in approvals
			.iter()
			.filter(|item| item.enabled && item.scope == ApprovalScope::Deployment)
		{
			let mut item = item.clone();
			item.expected_revision = item
				.expected_node_revisions
				.get(node.as_str())
				.copied()
				.unwrap_or(0);
			if let Some(restriction) = &mut item.restriction {
				restriction.subjects = restriction.subjects.iter().map(&translate).collect();
			}
			selected.push(item);
		}
		let subjects = extra_subjects.iter().map(&translate).collect();
		access.set_node(node);
		aidash_application::authorization::catalog::approvals::approve_with_satisfied(
			&mut TenantCatalog {
				access,
				node: address,
				deployment_scope_orchestrated: true,
			},
			&selected,
			satisfied_other_nodes,
			&subjects,
		)
		.await?;
	}
	access.set_node(source.clone());
	Ok(source_bindings)
}

/// Resolve real people with active Mappings and verify their current review authority.
async fn request_mail_recipients(
	f: &crate::federation::Federation,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
	root: &crate::registry::EntityRef,
	digest: &str,
) -> Result<std::collections::BTreeSet<String>> {
	use crate::apps::identity::models::{DashboardIdentity, DashboardMapping};
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let candidates = f
		.store
		.pool
		.scoped(&scope, async |tx| {
			let mappings = DashboardMapping::objects()
				.filter(DashboardMapping::field_tenant().eq(tenant.to_owned()))
				.filter(DashboardMapping::field_enabled().eq(true))
				.all_with_executor(&mut **tx)
				.await
				.map_err(FrameworkError::from)?;
			let mut candidates = Vec::new();
			for mapping in mappings {
				if let Some(person) = DashboardIdentity::objects()
					.filter(DashboardIdentity::field_id().eq(mapping.identity_id()))
					.all_with_executor(&mut **tx)
					.await
					.map_err(FrameworkError::from)?
					.pop() && person.disabled_at.is_none()
					&& person.approval_request_mail
					&& let Some(email) = person.verified_email
				{
					candidates.push((mapping.subject.clone(), mapping.credential_id(), email));
				}
			}
			Ok::<_, Error>(candidates)
		})
		.await?;
	let mut recipients = std::collections::BTreeSet::new();
	for (subject, credential_id, email) in candidates {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id,
			tenant: tenant.to_owned(),
			subject,
		};
		let mut access = match Access::begin_proof(&f.store, &identity).await {
			Ok(access) => access,
			Err(_) => continue,
		};
		let eligible =
			match request_review_for(&mut access, &f.store, &f.config.deployment, node, root).await
			{
				Ok(review) => {
					review.closure_digest == digest
						&& !review.ready && review.items.iter().any(ReviewItem::awaits_approver)
				}
				Err(_) => false,
			};
		access.finish(Ok(())).await?;
		if eligible {
			recipients.insert(email);
		}
	}
	Ok(recipients)
}

fn request_review_summary(review: &ApprovalReview) -> CatalogRequestReview {
	CatalogRequestReview {
		ready: review.ready,
		hidden: review.hidden as u64,
		total: review.items.len() as u64,
		approvable: review.items.iter().filter(|item| item.can_approve).count() as u64,
	}
}

fn request_view(
	request: &ApprovalRequest,
	review: Option<&ApprovalReview>,
) -> CatalogApprovalRequestView {
	// A read derives the effective state from the current review without
	// writing: a stale closure reads as stale and a ready review as fulfilled.
	// Only an exclusive write path persists those derived states.
	let state = if request.state.is_open() {
		match review {
			Some(review) if review.closure_digest != request.closure_digest => RequestState::Stale,
			Some(review) if review.ready => RequestState::Fulfilled,
			_ => request.state,
		}
	} else {
		request.state
	};
	let next_action = match state {
		RequestState::Fulfilled => CatalogRequestNextAction::Fulfilled,
		RequestState::Declined => CatalogRequestNextAction::Declined,
		RequestState::Stale => CatalogRequestNextAction::Stale,
		RequestState::Withdrawn => CatalogRequestNextAction::Withdrawn,
		RequestState::Open if review.is_some_and(|review| review.ready) => {
			CatalogRequestNextAction::Ready
		}
		RequestState::Open => CatalogRequestNextAction::WaitingForApprover,
	};
	CatalogApprovalRequestView {
		id: request.id,
		node: request.node.clone(),
		root: request.root.clone(),
		requester: request.requester.clone(),
		reason: request.reason.clone(),
		state,
		decline_reason: (state == RequestState::Declined)
			.then(|| request.decline_reason.clone())
			.flatten(),
		closure_digest: request.closure_digest.clone(),
		created_at: request.created_at,
		updated_at: request.updated_at,
		revision: request.revision,
		next_action,
		review: review.map(request_review_summary),
	}
}

async fn refresh_requests_for_review(
	access: &mut Access,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
	root: &crate::registry::EntityRef,
	current: &ApprovalReview,
	actor: &AdministrationActor,
) -> Result<()> {
	let now = database_time(&mut *access.tx).await?;
	for mut request in requests::open_by_root(&mut access.tx, tenant, node, root).await? {
		if request.closure_digest != current.closure_digest {
			request.stale(now)?;
			requests::save(&mut access.tx, &request).await?;
		} else if current.ready {
			request.fulfil(&actor.decision_actor(), now)?;
			requests::save(&mut access.tx, &request).await?;
			record_request_event(
				access,
				actor,
				CATALOG_REQUEST_FULFIL,
				tenant,
				&request.requester,
			)
			.await?;
		}
	}
	Ok(())
}

/// Persist the outcome of every open request on `node` after an approval write
/// that is not a review submission, so a request completed through the single,
/// batch or Marketplace approval endpoint stays fulfilled instead of tracking
/// current readiness. A root this approver cannot review keeps its derived
/// read state until a later write path can see it. A root whose review is
/// itself invalid (for example a closure past the review limit) is skipped the
/// same way, so one unreviewable request never rolls back an unrelated
/// approval; storage and transaction failures still propagate.
pub(crate) async fn refresh_requests_on_node(
	access: &mut Access,
	store: &crate::store::Store,
	deployment: &aidash_domain::nodes::DeploymentId,
	identity: &SubjectIdentity,
	node: &aidash_domain::nodes::NodeName,
) -> Result<()> {
	const BATCH: u64 = 256;
	let mut roots = std::collections::BTreeSet::new();
	let mut offset = 0u64;
	loop {
		let rows = requests::open_page(&mut access.tx, &identity.tenant, Some(node), offset, BATCH)
			.await?;
		let count = rows.len() as u64;
		roots.extend(
			rows.into_iter()
				.map(|request| (request.root.id, request.root.version)),
		);
		offset += count;
		if count < BATCH {
			break;
		}
	}
	let actor = request_audit_actor(identity);
	for (id, version) in roots {
		let root = crate::registry::EntityRef { id, version };
		let current = match request_review_for(access, store, deployment, node, &root).await {
			Ok(current) => current,
			Err(Error::Forbidden | Error::NotFound(_) | Error::Invalid(_)) => continue,
			Err(error) => return Err(error),
		};
		refresh_requests_for_review(access, &identity.tenant, node, &root, &current, &actor)
			.await?;
	}
	Ok(())
}

fn actor_principal(actor: &Actor) -> aidash_domain::identity::Principal {
	match actor {
		Actor::DeploymentAdministrator => {
			aidash_domain::identity::Principal::DeploymentAdministrator
		}
		Actor::Subject(identity) => aidash_domain::identity::Principal::Subject {
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		},
	}
}

impl PolicyAdministration {
	fn administrations(&self) -> TenantAdministrations {
		TenantAdministrations {
			runtime: self.runtime.clone(),
		}
	}

	/// The Tenant's Catalog Approvals, under `registry.read`. A Deployment
	/// Administrator reads none (ADR 0029).
	pub(crate) async fn catalog(
		&self,
		actor: Actor,
		tenant: String,
		node: aidash_domain::nodes::NodeName,
	) -> Result<Vec<Binding>> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			let resource = access.resource(
				"registry",
				aidash_domain::identity::tenant_administration::COLLECTION,
				serde_json::json!({}),
			);
			access.require(&resource, "registry.read").await?;
			crate::apps::identity::repositories::nodes::get(
				&mut *access.tx,
				&tenant,
				node.as_str(),
			)
			.await?;
			Ok(AuthorizationCatalog::objects()
				.filter(AuthorizationCatalog::field_tenant().eq(identity.tenant.clone()))
				.filter(AuthorizationCatalog::field_node().eq(node.as_str().to_owned()))
				.order_by(&["entry_id", "entry_version"])
				.all_with_executor(&mut **access.tx)
				.await
				.map_err(FrameworkError::from)?
				.into_iter()
				.map(|row| Binding {
					tenant: row.tenant,
					node: node.clone(),
					entry_id: row.entry_key,
					entry_version: row.entry_version,
					enabled: row.enabled,
					revision: row.revision,
					restriction: row
						.restriction
						.and_then(|value| serde_json::from_value(value.0).ok()),
				})
				.collect())
		}
		.await;
		access.finish(result).await
	}
	/// The reviewed closure of one root on one Node.
	pub(crate) async fn review(
		&self,
		actor: Actor,
		tenant: String,
		query: CatalogReviewQuery,
	) -> Result<ApprovalReview> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			let node = query.node;
			crate::apps::identity::repositories::nodes::get(
				&mut **access.tx,
				&tenant,
				node.as_str(),
			)
			.await?;
			access.set_node(node.clone());
			let root = review_entry(&query.entry)?;
			let mut scope = ReviewScope {
				access: &mut access,
				include_node_revisions: true,
				node: f.config.deployment.node(node),
				validation: crate::bootstrap::registry_validation_for(&f.store),
			};
			Ok(
				aidash_application::authorization::catalog::review::review(&mut scope, &root)
					.await?,
			)
		}
		.await;
		access.finish(result).await
	}

	/// What withdrawing one approved version affects on one Node. Reading it
	/// needs the right to withdraw it; the withdrawal itself is never blocked.
	pub(crate) async fn withdrawal_impact(
		&self,
		actor: Actor,
		tenant: String,
		query: CatalogImpactQuery,
	) -> Result<aidash_application::authorization::catalog::withdrawal::WithdrawalImpact> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			let node = query.node;
			crate::apps::identity::repositories::nodes::get(
				&mut **access.tx,
				&tenant,
				node.as_str(),
			)
			.await?;
			access.set_node(node.clone());
			let reference = crate::registry::EntityRef {
				id: query.id,
				version: query.version,
			};
			let mut scope =
				crate::apps::identity::repositories::catalog_withdrawal::WithdrawalScope {
					access: &mut access,
					node: f.config.deployment.node(node),
				};
			Ok(
				aidash_application::authorization::catalog::withdrawal::impact(
					&mut scope, &reference,
				)
				.await?,
			)
		}
		.await;
		access.finish(result).await
	}

	/// Decide the reviewed subset and activate its Marketplace copies in the
	/// same transaction. The closure is recomputed before any write.
	pub(crate) async fn submit_review(
		&self,
		actor: Actor,
		tenant: String,
		input: ApprovalReviewSubmission,
	) -> Result<CatalogReviewResult> {
		let f = self.runtime.clone();
		let principal = actor_principal(&actor);
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin_change(
			&f.store,
			&identity,
			&[aidash_application::authorization::catalog::approvals::CATALOG_APPROVE],
		)
		.await?;
		let result = async {
			let node = input.node.clone();
			crate::apps::identity::repositories::nodes::get(
				&mut **access.tx,
				&tenant,
				node.as_str(),
			)
			.await?;
			access.set_node(node.clone());
			let node_address = f.config.deployment.node(node.clone());
			let current = {
				let mut scope = ReviewScope {
					access: &mut access,
					include_node_revisions: false,
					node: node_address.clone(),
					validation: crate::bootstrap::registry_validation_for(&f.store),
				};
				aidash_application::authorization::catalog::review::review(&mut scope, &input.root)
					.await?
			};
			if let Some(request_id) = input.request {
				let request = requests::by_id(&mut access.tx, &tenant, request_id)
					.await?
					.ok_or_else(|| Error::Conflict("APPROVAL_REQUEST_STALE".into()))?;
				if !request.state.is_open()
					|| request.closure_digest != current.closure_digest
					|| request.root != input.root
					|| request.node != input.node
				{
					return Err(Error::Conflict("APPROVAL_REQUEST_STALE".into()));
				}
			}
			let plan = aidash_application::authorization::catalog::review::plan(&current, &input)?;
			let mut extra_subjects = std::collections::BTreeSet::new();
			// A dependency restriction may name the reviewed agent root before
			// that root is approved: the review's secure default proposes it, and
			// the root's approval may need the dependency first. Naming it only
			// passes restriction validation; the agent becomes a policy subject
			// when its own approval succeeds.
			if current
				.items
				.iter()
				.any(|item| item.reference == input.root && item.kind == "agent")
			{
				extra_subjects.insert(aidash_domain::qualified_agent(
					&node_address,
					&input.root.id,
					&input.root.version,
				));
			}
			// The same holds for the closure agents the review names as an item's
			// consumers: a delegated agent's dependency may be approved first,
			// with the proposed restriction naming that agent (ADR 0095).
			for item in &current.items {
				extra_subjects.extend(item.consumers.iter().cloned());
			}
			for approval in &plan.approvals {
				if let Some(item) = current
					.items
					.iter()
					.find(|item| item.reference == approval.entry)
					&& item.kind == "agent"
				{
					extra_subjects.insert(aidash_domain::qualified_agent(
						&node_address,
						&item.reference.id,
						&item.reference.version,
					));
				}
			}
			for activation in &plan.installations {
				if let Some(item) = current.items.iter().find(|item| {
					item.installation.as_ref().is_some_and(|installed| {
						installed.installation == activation.installation
							&& installed.revision == activation.revision
					})
				}) && item.kind == "agent"
				{
					extra_subjects.insert(aidash_domain::qualified_agent(
						&node_address,
						&item.reference.id,
						&item.reference.version,
					));
				}
			}
			for activation in plan.installations.iter().chain(&plan.restrictions) {
				let Some(restriction) = &activation.restriction else {
					continue;
				};
				restriction.validate()?;
				for subject in &restriction.subjects {
					if !access.snapshot.bundle.subjects.contains_key(subject)
						&& !extra_subjects.contains(subject)
					{
						return Err(Error::Invalid(format!(
							"CATALOG_RESTRICTION_UNKNOWN_SUBJECT: {subject}"
						)));
					}
				}
				for group in &restriction.groups {
					if !access
						.snapshot
						.bundle
						.groups
						.get(group)
						.is_some_and(|group| group.assignable)
					{
						return Err(Error::Invalid(format!(
							"CATALOG_RESTRICTION_NOT_ASSIGNABLE: {group}"
						)));
					}
				}
			}
			// A local Deployment-wide approval may depend on an installation in this
			// submission only if that installation is also approved on every Node.
			let deployment_activations =
				plan.installations
					.iter()
					.filter_map(|activation| {
						current.items.iter().find(|item| {
							item.installation.as_ref().is_some_and(|installed| {
								installed.installation == activation.installation
									&& installed.revision == activation.revision
							}) && activation.scope.or(item.scope).unwrap_or(
								aidash_domain::identity::catalog::ApprovalScope::Deployment,
							) == aidash_domain::identity::catalog::ApprovalScope::Deployment
						})
					})
					.map(|item| (item.reference.id.clone(), item.reference.version.clone()))
					.collect();
			// An activation-only submission has no local approval to decide.
			let bindings = if plan.approvals.is_empty() {
				vec![]
			} else {
				approve_scoped(
					&mut access,
					&f.config.deployment,
					&node,
					&plan.approvals,
					&plan.satisfied_by_activation,
					&deployment_activations,
					&extra_subjects,
				)
				.await?
			};
			// Restriction-only edits never activate, so a newer staged revision
			// does not block editing the active one (#590).
			if !plan.restrictions.is_empty() {
				let set = aidash_application::authorization::catalog::review::restriction_set(
					&tenant, &node, &current, &plan,
				)?;
				aidash_application::marketplace::operations::restriction::restrict_active(
					&mut crate::bootstrap::marketplace_tenant_activation_scope_at(
						&f.store,
						&mut access.tx,
						principal.clone(),
						tenant.clone(),
						node.clone(),
					),
					&set,
				)
				.await?;
			}
			let mut installations = vec![];
			if !plan.installations.is_empty() {
				let set = aidash_application::authorization::catalog::review::approval_set(
					&tenant, &node, &current, &plan,
				)?;
				installations =
					aidash_application::marketplace::operations::approval_set::approve_and_activate(
						&mut crate::bootstrap::marketplace_tenant_activation_scope_at(
							&f.store,
							&mut access.tx,
							principal,
							tenant.clone(),
							node.clone(),
						),
						&crate::bootstrap::registry_validation_for(&f.store),
						&set,
						f.store.deployment.as_str(),
					)
					.await?;
				let mut entries = vec![];
				for activation in &plan.installations {
					if let Some(item) = current.items.iter().find(|item| {
						item.installation.as_ref().is_some_and(|installed| {
							installed.installation == activation.installation
								&& installed.revision == activation.revision
						})
					}) && item.kind == "agent"
					{
						entries.push(
							crate::apps::registry::models::Definition::read_in(
								&mut *access.tx,
								&item.reference.id,
								&item.reference.version,
							)
							.await?,
						);
					}
				}
				crate::apps::identity::repositories::catalog_approvals::admit_agents(
					&mut access,
					&node_address,
					&entries,
				)
				.await?;
				let mut scoped = Vec::new();
				for activation in &plan.installations {
					let item = current
						.items
						.iter()
						.find(|item| {
							item.installation.as_ref().is_some_and(|installed| {
								installed.installation == activation.installation
									&& installed.revision == activation.revision
							})
						})
						.ok_or_else(|| {
							Error::Conflict("reviewed installation disappeared".into())
						})?;
					let row = AuthorizationCatalog::objects()
						.filter(AuthorizationCatalog::field_tenant().eq(tenant.clone()))
						.filter(AuthorizationCatalog::field_node().eq(node.as_str().to_owned()))
						.filter(
							AuthorizationCatalog::field_entry_key().eq(item.reference.id.clone()),
						)
						.filter(
							AuthorizationCatalog::field_entry_version()
								.eq(item.reference.version.clone()),
						)
						.all_with_executor(&mut **access.tx)
						.await
						.map_err(FrameworkError::from)?
						.pop()
						.ok_or_else(|| Error::Conflict("activated approval disappeared".into()))?;
					scoped.push(
						aidash_application::authorization::catalog::approvals::Approval {
							expected_node_revisions: activation.expected_node_revisions.clone(),
							entry: item.reference.clone(),
							enabled: true,
							expected_revision: row.revision,
							carry_over: activation.carry_over.or(item.carry_over).unwrap_or(true),
							scope: activation.scope.or(item.scope).unwrap_or(
								aidash_domain::identity::catalog::ApprovalScope::Deployment,
							),
							provider_warning_acknowledged: activation.provider_warning_acknowledged,
							groups: None,
							restriction: row
								.restriction
								.map(|value| serde_json::from_value(value.0))
								.transpose()?,
						},
					);
				}
				approve_scoped(
					&mut access,
					&f.config.deployment,
					&node,
					&scoped,
					&plan.satisfied_by_activation,
					&deployment_activations,
					&extra_subjects,
				)
				.await?;
			}
			let after = {
				let mut scope = ReviewScope {
					access: &mut access,
					include_node_revisions: false,
					node: node_address.clone(),
					validation: crate::bootstrap::registry_validation_for(&f.store),
				};
				aidash_application::authorization::catalog::review::review(&mut scope, &input.root)
					.await?
			};
			refresh_requests_for_review(
				&mut access,
				&tenant,
				&node,
				&input.root,
				&after,
				&request_audit_actor(&identity),
			)
			.await?;
			Ok(CatalogReviewResult {
				bindings,
				installations,
			})
		}
		.await;
		access.finish(result).await
	}

	/// Ask an approver to review one root. A request grants nothing and one
	/// open request per requester, root and Node is idempotent.
	pub(crate) async fn create_request(
		&self,
		actor: Actor,
		tenant: String,
		input: CatalogRequestInput,
	) -> Result<CatalogApprovalRequestView> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let requester = identity.subject.clone();
		// A request changes no policy, and `requests::lock` serializes requests,
		// so any member can make one without holding up the Tenant's work.
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			crate::apps::identity::repositories::nodes::get(
				&mut **access.tx,
				&tenant,
				input.node.as_str(),
			)
			.await?;
			let current = request_review_for(
				&mut access,
				&f.store,
				&f.config.deployment,
				&input.node,
				&input.root,
			)
			.await?;
			if current.ready {
				return Err(Error::Conflict("CATALOG_APPROVAL_REQUEST_READY".into()));
			}
			requests::lock(&mut access.tx, &tenant).await?;
			if let Some(mut existing) = requests::open_for(
				&mut access.tx,
				&tenant,
				&requester,
				&input.root,
				&input.node,
			)
			.await?
			{
				if existing.closure_digest == current.closure_digest {
					return Ok((request_view(&existing, Some(&current)), false));
				}
				let now = database_time(&mut *access.tx).await?;
				existing.stale(now)?;
				requests::save(&mut access.tx, &existing).await?;
			}
			let now = database_time(&mut *access.tx).await?;
			let request = ApprovalRequest::new(NewCatalogApprovalRequest {
				id: Uuid::new_v4(),
				tenant: tenant.clone(),
				node: input.node.clone(),
				root: input.root.clone(),
				requester: requester.clone(),
				reason: input.reason,
				closure_digest: current.closure_digest.clone(),
				now,
			})?;
			requests::insert(&mut access.tx, &request).await?;
			record_request_event(
				&mut access,
				&request_audit_actor(&identity),
				CATALOG_REQUEST_CREATE,
				&tenant,
				&request.requester,
			)
			.await?;
			Ok((request_view(&request, Some(&current)), true))
		}
		.await;
		let (view, created) = access.finish(result).await?;
		if created
			&& f.config
				.smtp
				.as_ref()
				.is_some_and(|smtp| !smtp.host.trim().is_empty())
		{
			let mail_node = view.node.clone();
			let mail_root = view.root.clone();
			let mail_digest = view.closure_digest.clone();
			let mail_reason = view.reason.clone();
			let mail_id = view.id;
			tokio::spawn(async move {
				use aidash_application::ports::catalog_mail::{
					CatalogRequestMailer, CatalogRequestNotice,
				};
				let recipients =
					request_mail_recipients(&f, &tenant, &mail_node, &mail_root, &mail_digest)
						.await;
				match recipients {
					Ok(recipients) => {
						if let Ok(mut link) = reqwest::Url::parse(&f.config.dashboard_origin) {
							link.set_path("/settings");
							link.query_pairs_mut()
								.append_pair("view", "registry")
								.append_pair("tenant", &tenant)
								.append_pair("node", mail_node.as_str())
								.append_pair("request", &mail_id.to_string());
							let notice = CatalogRequestNotice {
								definition: mail_root.clone(),
								node: mail_node.clone(),
								reason: mail_reason.clone(),
								review_link: link.to_string(),
							};
							let mailer = crate::apps::identity::services::catalog_mail::CatalogMailer::configured(f.config.smtp.as_ref());
							for recipient in recipients {
								if let Err(error) = mailer.send(&recipient, &notice).await {
									tracing::warn!(%error, "catalog request mail delivery failed");
								}
							}
						} else {
							tracing::warn!("catalog request mail has an invalid dashboard origin");
						}
					}

					Err(error) => {
						tracing::warn!(%error, "catalog request mail recipient lookup failed")
					}
				}
			});
		}
		Ok(view)
	}

	/// Withdraw one's own open request.
	pub(crate) async fn withdraw_request(
		&self,
		actor: Actor,
		tenant: String,
		id: Uuid,
	) -> Result<CatalogApprovalRequestView> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			requests::lock(&mut access.tx, &tenant).await?;
			let mut request = requests::by_id(&mut access.tx, &tenant, id)
				.await?
				.ok_or_else(|| Error::NotFound("catalog approval request".into()))?;
			if request.requester != identity.subject {
				return Err(Error::Forbidden);
			}
			let now = database_time(&mut *access.tx).await?;
			request.withdraw(&identity.subject, now)?;
			requests::save(&mut access.tx, &request).await?;
			record_request_event(
				&mut access,
				&request_audit_actor(&identity),
				CATALOG_REQUEST_WITHDRAW,
				&tenant,
				&request.requester,
			)
			.await?;
			Ok(request_view(&request, None))
		}
		.await;
		access.finish(result).await
	}

	/// Decline an open request as someone who could approve at least one item
	/// of its current review.
	pub(crate) async fn decline_request(
		&self,
		actor: Actor,
		tenant: String,
		id: Uuid,
		input: CatalogRequestDecline,
	) -> Result<CatalogApprovalRequestView> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			requests::lock(&mut access.tx, &tenant).await?;
			let mut request = requests::by_id(&mut access.tx, &tenant, id)
				.await?
				.ok_or_else(|| Error::NotFound("catalog approval request".into()))?;
			if !request.state.is_open() {
				return Err(Error::Conflict(
					"catalog approval request is already final".into(),
				));
			}
			let current = match request_review_for(
				&mut access,
				&f.store,
				&f.config.deployment,
				&request.node,
				&request.root,
			)
			.await
			{
				Ok(current) => current,
				Err(Error::Forbidden | Error::NotFound(_)) => return Err(Error::Forbidden),
				Err(error) => return Err(error),
			};
			if current.ready {
				return Err(Error::Conflict("CATALOG_APPROVAL_REQUEST_READY".into()));
			}
			if request.closure_digest != current.closure_digest {
				// Reads derive stale without writing. Returning the conflict
				// keeps this transaction atomic; persisting stale here would be
				// rolled back with the error.
				return Err(Error::Conflict("CATALOG_APPROVAL_REQUEST_STALE".into()));
			}
			if !current.items.iter().any(ReviewItem::awaits_approver) {
				return Err(Error::Forbidden);
			}
			let now = database_time(&mut *access.tx).await?;
			request.decline(&identity.subject, &input.reason, now)?;
			requests::save(&mut access.tx, &request).await?;
			record_request_event(
				&mut access,
				&request_audit_actor(&identity),
				CATALOG_REQUEST_DECLINE,
				&tenant,
				&request.requester,
			)
			.await?;
			Ok(request_view(&request, Some(&current)))
		}
		.await;
		access.finish(result).await
	}

	/// The requester's own requests. Reads derive stale/fulfilled without writing.
	pub(crate) async fn mine_requests(
		&self,
		actor: Actor,
		tenant: String,
		page: CatalogRequestPage,
	) -> Result<CatalogApprovalRequestPage> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let requester = identity.subject.clone();
		let limit = page.limit.clamp(1, 100);
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			let rows = requests::mine(
				&mut access.tx,
				&tenant,
				&requester,
				page.node.as_ref(),
				page.offset,
				limit + 1,
			)
			.await?;
			let more = rows.len() as u64 > limit;
			let mut views = vec![];
			for request in rows.into_iter().take(limit as usize) {
				// A read never writes. It derives stale/fulfilled for display;
				// the next exclusive write path persists that state.
				let review = if request.state.is_open() {
					match request_review_for(
						&mut access,
						&f.store,
						&f.config.deployment,
						&request.node,
						&request.root,
					)
					.await
					{
						Ok(current) => Some(current),
						// An unreviewable root still lists, without a review.
						Err(Error::Forbidden | Error::NotFound(_) | Error::Invalid(_)) => None,
						Err(error) => return Err(error),
					}
				} else {
					None
				};
				views.push(request_view(&request, review.as_ref()));
			}
			Ok(CatalogApprovalRequestPage {
				requests: views,
				next_offset: more.then_some(page.offset + limit),
			})
		}
		.await;
		access.finish(result).await
	}

	/// Open requests where this principal can approve at least one visible
	/// item. Hidden items contribute only to the count.
	pub(crate) async fn pending_requests(
		&self,
		actor: Actor,
		tenant: String,
		page: CatalogRequestPage,
	) -> Result<CatalogApprovalRequestPage> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let limit = page.limit.clamp(1, 100);
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			// The pending filter is in memory, so scan open requests in pages
			// until the requested window is filled or the rows are exhausted.
			// This keeps pagination correct past any fixed batch size.
			const BATCH: u64 = 256;
			let mut offset = 0u64;
			let mut matched = 0u64;
			let mut views = vec![];
			let mut reviews: std::collections::BTreeMap<(String, String, String), ApprovalReview> =
				std::collections::BTreeMap::new();
			loop {
				let rows =
					requests::open_page(&mut access.tx, &tenant, page.node.as_ref(), offset, BATCH)
						.await?;
				let count = rows.len() as u64;
				for request in rows {
					let key = (
						request.node.as_str().to_owned(),
						request.root.id.clone(),
						request.root.version.clone(),
					);
					let current = if let Some(current) = reviews.get(&key) {
						current.clone()
					} else {
						let current = match request_review_for(
							&mut access,
							&f.store,
							&f.config.deployment,
							&request.node,
							&request.root,
						)
						.await
						{
							Ok(current) => current,
							// Nobody can act on an unreviewable root, so it
							// does not fail the whole queue.
							Err(Error::Forbidden | Error::NotFound(_) | Error::Invalid(_)) => {
								continue;
							}
							Err(error) => return Err(error),
						};
						reviews.insert(key, current.clone());
						current
					};
					// Derived, never written: a stale closure or a ready review
					// leaves the pending list.
					if request.closure_digest != current.closure_digest || current.ready {
						continue;
					}
					if current.items.iter().any(ReviewItem::awaits_approver) {
						if matched >= page.offset {
							views.push(request_view(&request, Some(&current)));
						}
						matched += 1;
						if views.len() as u64 > limit {
							break;
						}
					}
				}
				offset += count;
				if views.len() as u64 > limit || count < BATCH {
					break;
				}
			}
			let more = views.len() as u64 > limit;
			let requests = views.into_iter().take(limit as usize).collect();
			Ok(CatalogApprovalRequestPage {
				requests,
				next_offset: more.then_some(page.offset + limit),
			})
		}
		.await;
		access.finish(result).await
	}

	/// Count all eligible pending requests across the Tenant's Nodes.
	pub(crate) async fn pending_request_count(
		&self,
		actor: Actor,
		tenant: String,
	) -> Result<CatalogPendingCount> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			const BATCH: u64 = 256;
			let live_nodes =
				crate::apps::identity::repositories::nodes::list_live(&mut **access.tx, &tenant)
					.await?
					.into_iter()
					.map(|node| node.name)
					.collect::<std::collections::BTreeSet<_>>();
			let mut offset = 0;
			let mut count = 0;
			let mut reviews =
				std::collections::BTreeMap::<(String, String, String), ApprovalReview>::new();
			loop {
				let rows =
					requests::open_page(&mut access.tx, &tenant, None, offset, BATCH).await?;
				let len = rows.len() as u64;
				for request in rows {
					if !live_nodes.contains(&request.node) {
						continue;
					}
					let key = (
						request.node.as_str().to_owned(),
						request.root.id.clone(),
						request.root.version.clone(),
					);
					let review = if let Some(review) = reviews.get(&key) {
						review.clone()
					} else {
						let review = match request_review_for(
							&mut access,
							&f.store,
							&f.config.deployment,
							&request.node,
							&request.root,
						)
						.await
						{
							Ok(review) => review,
							Err(Error::Forbidden | Error::NotFound(_) | Error::Invalid(_)) => {
								continue;
							}
							Err(error) => return Err(error),
						};
						reviews.insert(key, review.clone());
						review
					};
					if request.closure_digest == review.closure_digest
						&& !review.ready && review.items.iter().any(ReviewItem::awaits_approver)
					{
						count += 1;
					}
				}
				offset += len;
				if len < BATCH {
					break;
				}
			}
			Ok(CatalogPendingCount { count })
		}
		.await;
		access.finish(result).await
	}

	pub(crate) async fn catalog_mail_preference(
		&self,
		actor: Actor,
		tenant: String,
		change: Option<CatalogMailPreference>,
	) -> Result<CatalogMailPreference> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let person = identity
			.http_session
			.as_ref()
			.ok_or(Error::Forbidden)?
			.identity_id;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			use crate::apps::identity::models::DashboardIdentity;
			let mut person = DashboardIdentity::objects()
				.filter(DashboardIdentity::field_id().eq(person))
				.all_with_executor(&mut **access.tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or(Error::Forbidden)?;
			if let Some(change) = change {
				person.approval_request_mail = change.enabled;
				use reinhardt::query::{
					Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
				};
				let query = Query::update()
					.table(Alias::new("dashboard_identities"))
					.value(Alias::new("approval_request_mail"), change.enabled)
					.and_where(Expr::col(Alias::new("id")).eq(person.id.to_string()))
					.to_string(PostgresQueryBuilder);
				crate::database::native::query(&query)
					.execute(&mut **access.tx)
					.await?;
			}
			Ok(CatalogMailPreference {
				enabled: person.approval_request_mail,
			})
		}
		.await;
		access.finish(result).await
	}

	pub(crate) async fn set_catalog(
		&self,
		actor: Actor,
		tenant: String,
		input: CatalogInput,
	) -> Result<Binding> {
		let withdraw_carried = input.withdraw_carried;
		let stop_carrying = input.stop_carrying;
		let options = (input.carry_over, input.scope);
		if input.enabled && withdraw_carried {
			return Err(Error::Invalid(
				"withdrawal options require a withdrawal".into(),
			));
		}
		let approval = aidash_application::authorization::catalog::approvals::Approval {
			expected_node_revisions: input.expected_node_revisions,
			carry_over: input.carry_over.unwrap_or(true),
			scope: input
				.scope
				.unwrap_or(aidash_domain::identity::catalog::ApprovalScope::Deployment),
			entry: input.entry,
			enabled: input.enabled,
			expected_revision: input.expected_revision,
			groups: input.groups,
			provider_warning_acknowledged: input.provider_warning_acknowledged,
			restriction: input.restriction,
		};
		Ok(self
			.approve_with_withdrawal(
				actor,
				tenant,
				input.node,
				vec![approval],
				withdraw_carried,
				stop_carrying,
				Some(vec![options]),
			)
			.await?
			.remove(0))
	}
	/// Approvals of one dependency closure in one transaction.
	pub(crate) async fn approve(
		&self,
		actor: Actor,
		tenant: String,
		node: aidash_domain::nodes::NodeName,
		inputs: Vec<CatalogApprovalInput>,
	) -> Result<Vec<Binding>> {
		let options = inputs
			.iter()
			.map(|item| (item.carry_over, item.scope))
			.collect();
		let approvals = inputs
			.into_iter()
			.map(
				|item| aidash_application::authorization::catalog::approvals::Approval {
					expected_node_revisions: item.expected_node_revisions,
					entry: item.entry,
					enabled: item.enabled,
					expected_revision: item.expected_revision,
					carry_over: item.carry_over.unwrap_or(true),
					scope: item
						.scope
						.unwrap_or(aidash_domain::identity::catalog::ApprovalScope::Deployment),
					provider_warning_acknowledged: item.provider_warning_acknowledged,
					groups: item.groups,
					restriction: item.restriction,
				},
			)
			.collect();
		self.approve_with_withdrawal(actor, tenant, node, approvals, false, false, Some(options))
			.await
	}

	async fn approve_with_withdrawal(
		&self,
		actor: Actor,
		tenant: String,
		node: aidash_domain::nodes::NodeName,
		mut approvals: Vec<aidash_application::authorization::catalog::approvals::Approval>,
		withdraw_carried: bool,
		stop_carrying: bool,
		options: Option<
			Vec<(
				Option<bool>,
				Option<aidash_domain::identity::catalog::ApprovalScope>,
			)>,
		>,
	) -> Result<Vec<Binding>> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin_change(
			&f.store,
			&identity,
			&[aidash_application::authorization::catalog::approvals::CATALOG_APPROVE],
		)
		.await?;
		if let Err(error) = crate::apps::identity::repositories::nodes::get(
			&mut **access.tx,
			&tenant,
			node.as_str(),
		)
		.await
		{
			return access.finish(Err(error)).await;
		}
		let result = async {
			if let Some(options) = options {
				for (first, (carry, scope)) in approvals.iter_mut().zip(options) {
					let row = AuthorizationCatalog::objects()
						.filter(AuthorizationCatalog::field_tenant().eq(tenant.clone()))
						.filter(AuthorizationCatalog::field_node().eq(node.as_str().to_owned()))
						.filter(AuthorizationCatalog::field_entry_key().eq(first.entry.id.clone()))
						.filter(
							AuthorizationCatalog::field_entry_version()
								.eq(first.entry.version.clone()),
						)
						.all_with_executor(&mut **access.tx)
						.await
						.map_err(FrameworkError::from)?
						.into_iter()
						.next();
					first.carry_over =
						carry.unwrap_or_else(|| row.as_ref().map_or(true, |r| r.carry_over));
					first.scope =
						scope.unwrap_or_else(|| match row.as_ref().map(|r| r.scope.as_str()) {
							Some("node") => aidash_domain::identity::catalog::ApprovalScope::Node,
							_ => aidash_domain::identity::catalog::ApprovalScope::Deployment,
						});
				}
			}
			let mut stop_descendants = Vec::new();
			if withdraw_carried || stop_carrying {
				let root = approvals
					.first()
					.cloned()
					.ok_or_else(|| Error::Invalid("missing withdrawal".into()))?;
				let rows = AuthorizationCatalog::objects()
					.filter(AuthorizationCatalog::field_tenant().eq(tenant.clone()))
					.filter(AuthorizationCatalog::field_node().eq(node.as_str().to_owned()))
					.filter(AuthorizationCatalog::field_entry_key().eq(root.entry.id.clone()))
					.all_with_executor(&mut **access.tx)
					.await
					.map_err(FrameworkError::from)?;
				let mut versions = std::collections::BTreeSet::from([root.entry.version.clone()]);
				loop {
					let before = versions.len();
					for row in &rows {
						if row
							.carried_from_version
							.as_ref()
							.is_some_and(|source| versions.contains(source))
						{
							versions.insert(row.entry_version.clone());
						}
					}
					if before == versions.len() {
						break;
					}
				}
				for row in rows.into_iter().filter(|row| {
					row.entry_version != root.entry.version && versions.contains(&row.entry_version)
				}) {
					if stop_carrying {
						stop_descendants.push(Binding {
							tenant: tenant.clone(),
							node: node.clone(),
							entry_id: row.entry_key.clone(),
							entry_version: row.entry_version.clone(),
							enabled: row.enabled,
							revision: row.revision,
							restriction: None,
						});
					}
					if !withdraw_carried || !row.enabled {
						continue;
					}
					approvals.push(
						aidash_application::authorization::catalog::approvals::Approval {
							expected_node_revisions: Default::default(),
							entry: crate::registry::EntityRef {
								id: row.entry_key,
								version: row.entry_version,
							},
							enabled: false,
							expected_revision: row.revision,
							carry_over: false,
							scope: aidash_domain::identity::catalog::ApprovalScope::Node,
							provider_warning_acknowledged: false,
							groups: None,
							restriction: None,
						},
					);
				}
			}
			let mut bindings = Vec::new();
			if stop_carrying
				&& !withdraw_carried
				&& approvals.len() == 1
				&& approvals[0].enabled
				&& approvals[0].groups.is_none()
				&& approvals[0].restriction.is_none()
			{
				use aidash_application::ports::catalog::CatalogApprovalScope;
				let decision = &approvals[0];
				access.set_node(node.clone());
				let definition =
					crate::apps::identity::repositories::catalog_approvals::definition(
						&mut access.tx,
						&decision.entry,
					)
					.await?;
				TenantCatalog {
					access: &mut access,
					node: f.config.deployment.node(node.clone()),
					deployment_scope_orchestrated: false,
				}
				.require(&definition, "catalog.approve")
				.await?;
				let row = AuthorizationCatalog::objects()
					.filter(AuthorizationCatalog::field_tenant().eq(tenant.clone()))
					.filter(AuthorizationCatalog::field_node().eq(node.as_str().to_owned()))
					.filter(AuthorizationCatalog::field_entry_key().eq(decision.entry.id.clone()))
					.filter(
						AuthorizationCatalog::field_entry_version()
							.eq(decision.entry.version.clone()),
					)
					.all_with_executor(&mut **access.tx)
					.await
					.map_err(FrameworkError::from)?
					.into_iter()
					.next()
					.filter(|row| row.enabled && row.revision == decision.expected_revision)
					.ok_or_else(|| Error::Conflict("catalog revision changed".into()))?;
				bindings.push(Binding {
					tenant: tenant.clone(),
					node: node.clone(),
					entry_id: row.entry_key,
					entry_version: row.entry_version,
					enabled: row.enabled,
					revision: row.revision,
					restriction: row
						.restriction
						.map(|value| serde_json::from_value(value.0))
						.transpose()?,
				});
			} else if withdraw_carried {
				for chunk in approvals.chunks(128) {
					bindings.extend(
						approve_scoped(
							&mut access,
							&f.config.deployment,
							&node,
							chunk,
							&std::collections::BTreeSet::new(),
							&std::collections::BTreeSet::new(),
							&std::collections::BTreeSet::new(),
						)
						.await?,
					);
				}
			} else {
				bindings = approve_scoped(
					&mut access,
					&f.config.deployment,
					&node,
					&approvals,
					&std::collections::BTreeSet::new(),
					&std::collections::BTreeSet::new(),
					&std::collections::BTreeSet::new(),
				)
				.await?;
			}

			if stop_carrying {
				let binding = bindings
					.first()
					.cloned()
					.ok_or_else(|| Error::Invalid("missing withdrawal".into()))?;
				bindings[0] = stop_carrying_revision(
					&mut access,
					&f.config.deployment,
					&node,
					&binding,
					&identity.subject,
				)
				.await?;
				for descendant in &mut stop_descendants {
					if let Some(updated) = bindings.iter().find(|b| {
						b.entry_id == descendant.entry_id
							&& b.entry_version == descendant.entry_version
					}) {
						descendant.revision = updated.revision;
					}
					*descendant = stop_carrying_revision(
						&mut access,
						&f.config.deployment,
						&node,
						descendant,
						&identity.subject,
					)
					.await?;
				}
			}
			refresh_requests_on_node(
				&mut access,
				&f.store,
				&f.config.deployment,
				&identity,
				&node,
			)
			.await?;
			Ok(bindings)
		}
		.await;
		access.finish(result).await
	}
	pub(crate) async fn issue_credential(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: CredentialInput,
	) -> Result<IssuedCredential> {
		let f = self.runtime.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.issue_credential(&f.store.pool, &input.subject, input.expires_in_seconds)
					.await
			})
			.await
	}
	pub(crate) async fn credentials(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: CredentialPage,
	) -> Result<Vec<Credential>> {
		let f = self.runtime.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.credentials(&f.store.pool, page.offset, page.mine, page.active)
					.await
			})
			.await
	}
	pub(crate) async fn revoke_credential(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		(tenant, id): (String, Uuid),
	) -> Result<(reinhardt::StatusCode, CredentialRevocation)> {
		let f = self.runtime.clone();
		let credential = self
			.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				// Revocation keeps its own pool so a busy API pool cannot delay it.
				administrator
					.revoke_credential(&f.store.control_pool, id)
					.await
			})
			.await?;
		let pending_transactions =
			crate::transactions::authority::pending(&f, &tenant, Some(id)).await?;
		let status = if pending_transactions.is_empty() {
			reinhardt::StatusCode::OK
		} else {
			reinhardt::StatusCode::ACCEPTED
		};
		Ok((
			status,
			CredentialRevocation {
				credential,
				pending_transactions,
			},
		))
	}
	pub(crate) async fn snapshot(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<Snapshot> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator) {
			return control_service(f).snapshot(&tenant).await;
		}
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator.policy_snapshot(&f.store.pool).await
			})
			.await
	}
	pub(crate) async fn replace(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: AuthorizationUpdate,
	) -> Result<(reinhardt::StatusCode, PolicyReplacement)> {
		let f = self.runtime.clone();
		// The replacement itself refuses a deleted Tenant too, but its error
		// crosses the authorization port as a plain refusal.
		let deleted = f
			.store
			.pool
			.atomic(
				&aidash_application::tenancy::TenancyScope::tenant(&tenant)?,
				async |tx| {
					crate::apps::identity::repositories::tenant_deletions::is_deleted(tx, &tenant)
						.await
				},
			)
			.await?;
		if deleted {
			return Err(crate::Error::TenantDeleted);
		}
		let snapshot = if matches!(actor, Actor::DeploymentAdministrator) {
			// A browser session records which Deployment Administrator replaced it.
			let revision_actor = match &origin {
				Some(origin) => format!("deployment_administrator:oidc:{}", origin.identity_id),
				None => "deployment_administrator".to_owned(),
			};
			// A Tenant without a root record is refused with 404: only the Tenant
			// creation API makes Tenants (ADR 0050).
			control_service(f.clone())
				.replace(
					&tenant,
					input.expected_revision,
					input.bundle,
					&revision_actor,
				)
				.await?
		} else {
			self.administrations()
				.run(actor, origin, &tenant, async |administrator| {
					administrator
						.replace_policy(
							&f.store.control_pool,
							&AdministrationActor::TenantAdministrator(administrator.clone()),
							input.expected_revision,
							&input.bundle,
						)
						.await
				})
				.await?
		};
		let pending_transactions =
			crate::transactions::authority::pending(&f, &tenant, None).await?;
		let status = if pending_transactions.is_empty() {
			reinhardt::StatusCode::OK
		} else {
			reinhardt::StatusCode::ACCEPTED
		};
		Ok((
			status,
			PolicyReplacement {
				snapshot,
				pending_transactions,
			},
		))
	}
	pub async fn transaction_revocations(&self, tenant: String) -> Result<Vec<Uuid>> {
		crate::transactions::authority::pending(&self.runtime, &tenant, None).await
	}
	pub async fn evaluate(&self, tenant: String, input: Evaluation) -> Result<Decision> {
		let f = self.runtime.clone();
		control_service(f).evaluate(&tenant, &input).await
	}
	pub(crate) async fn simulate(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: Evaluation,
	) -> Result<Decision> {
		let f = self.runtime.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator.simulate(&f.store.pool, &input).await
			})
			.await
	}
	pub(crate) async fn revisions(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: AuthorizationPage,
	) -> Result<Vec<Value>> {
		let f = self.runtime.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.policy_revisions(&f.store.pool, page.after, page.limit)
					.await
			})
			.await
	}
	pub(crate) async fn decisions(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: AuthorizationPage,
	) -> Result<Vec<Value>> {
		let f = self.runtime.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.decisions(&f.store.pool, page.after, page.limit)
					.await
			})
			.await
	}
	/// Safety Stops of the Tenant's Runs (ADR 0101), for holders of
	/// `decision.read`. A Deployment Administrator's receipt grants no access to
	/// the Tenant's audit history, so an administrator is refused.
	pub(crate) async fn safety_stops(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: crate::apps::execution::serializers::research::HistoryPage,
	) -> Result<Vec<aidash_domain::safety_stop::Record>> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator) {
			// A deleted Tenant answers 410 to everyone before any other refusal.
			f.store
				.pool
				.atomic(
					&aidash_application::tenancy::TenancyScope::tenant(&tenant)?,
					async |tx| {
						crate::authorization::remote::deployment_administrator::tenant(tx, &tenant)
							.await
					},
				)
				.await?;
			return Err(Error::Forbidden);
		}
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.safety_stops(&f.store.pool, page.offset, page.limit)
					.await
			})
			.await
	}
}
