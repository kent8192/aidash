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
	AuthorizationPage, AuthorizationUpdate, CatalogApprovalRequestPage, CatalogApprovalRequestView,
	CatalogImpactQuery, CatalogInput, CatalogRequestDecline, CatalogRequestInput,
	CatalogRequestNextAction, CatalogRequestPage, CatalogRequestReview, CatalogReviewQuery,
	CatalogReviewResult, CredentialInput, CredentialPage, CredentialRevocation, PolicyReplacement,
};
use aidash_application::authorization::catalog::review::{
	ApprovalReview, ApprovalReviewSubmission, ReviewItem,
};
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
		node: deployment.node(node.clone()),
		validation: crate::bootstrap::registry_validation_for(store),
	};
	Ok(aidash_application::authorization::catalog::review::review(&mut scope, root).await?)
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
/// read state until a later write path can see it.
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
			Err(Error::Forbidden | Error::NotFound(_)) => continue,
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
		let mut access = Access::begin_exclusive(&f.store, &identity).await?;
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
			for activation in &plan.installations {
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
			// An activation-only submission has no local approval to decide.
			let bindings = if plan.approvals.is_empty() {
				vec![]
			} else {
				aidash_application::authorization::catalog::approvals::approve_with_satisfied(
					&mut TenantCatalog {
						access: &mut access,
						node: node_address.clone(),
					},
					&plan.approvals,
					&plan.satisfied_by_activation,
					&extra_subjects,
				)
				.await?
			};
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
			}
			let after = {
				let mut scope = ReviewScope {
					access: &mut access,
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
		let mut access = Access::begin_exclusive(&f.store, &identity).await?;
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
					return Ok(request_view(&existing, Some(&current)));
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
			Ok(request_view(&request, Some(&current)))
		}
		.await;
		access.finish(result).await
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
		let mut access = Access::begin_exclusive(&f.store, &identity).await?;
		let result = async {
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
		let mut access = Access::begin_exclusive(&f.store, &identity).await?;
		let result = async {
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
						Err(Error::Forbidden | Error::NotFound(_)) => None,
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
							Err(Error::Forbidden | Error::NotFound(_)) => continue,
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

	pub(crate) async fn set_catalog(
		&self,
		actor: Actor,
		tenant: String,
		input: CatalogInput,
	) -> Result<Binding> {
		let approval = aidash_application::authorization::catalog::approvals::Approval {
			entry: input.entry,
			enabled: input.enabled,
			expected_revision: input.expected_revision,
			groups: input.groups,
			restriction: input.restriction,
		};
		Ok(self
			.approve(actor, tenant, input.node, vec![approval])
			.await?
			.remove(0))
	}
	/// Approvals of one dependency closure in one transaction.
	pub(crate) async fn approve(
		&self,
		actor: Actor,
		tenant: String,
		node: aidash_domain::nodes::NodeName,
		approvals: Vec<aidash_application::authorization::catalog::approvals::Approval>,
	) -> Result<Vec<Binding>> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin_exclusive(&f.store, &identity).await?;
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
			let bindings = aidash_application::authorization::catalog::approvals::approve(
				&mut TenantCatalog {
					access: &mut access,
					node: f.config.deployment.node(node.clone()),
				},
				&approvals,
			)
			.await?;
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
