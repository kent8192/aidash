//! Native boundary adapters for sandbox admission, session management and background execution.
use super::*;
use crate::apps::registry::workbench::models::AgentTestSession;
use reinhardt::injectable;

/// Remove ordinary test payloads. The metadata and expiry marker remain.
pub async fn purge_expired(pool: &crate::database::native::Pool) -> Result<u64> {
	let scope = aidash_application::tenancy::TenancyScope::Deployment(
		aidash_application::tenancy::DeploymentScopeReason::Retention,
	);
	pool.atomic(&scope, async |tx| AgentTestSession::purge(tx).await)
		.await
}

pub use crate::apps::registry::workbench::serializers::test::{TestInput, TestLimits, TestSession};
use aidash_domain::registry::workbench::sandbox::TenantTestLimits;

#[derive(Clone)]
pub struct BehavioralTests {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_test(#[inject] runtime: Federation) -> BehavioralTests {
	BehavioralTests { runtime }
}

impl BehavioralTests {
	pub(crate) async fn answer_approval(
		&self,
		actor: Actor,
		id: Uuid,
		input: crate::apps::registry::workbench::serializers::test::TestApprovalAnswer,
	) -> Result<TestSession> {
		use crate::apps::registry::workbench::serializers::test::TestApprovalChoice;
		let Actor::Subject(identity) = &actor else {
			return Err(crate::Error::Forbidden);
		};
		let person = identity
			.http_session
			.as_ref()
			.map(|s| s.identity_id)
			.ok_or(crate::Error::Forbidden)?;
		let mut tx = self.runtime.store.pool.begin(&actor.tenancy()?).await?;
		let policy = tx.pool().dashboard_policy();
		let snapshot = identity
			.lock_native(&mut tx, false, policy.as_ref())
			.await?;
		let session = AgentTestSession::read(&mut tx, id, true).await?;
		let pending = &session.scenario["pending_approval"];
		if session.status != "running"
			|| session.scenario["starter_identity"] != serde_json::json!(person)
			|| session.scenario["starter_subject"] != identity.subject
			|| pending["id"] != serde_json::json!(input.approval_id)
			|| pending["state"] != "pending"
		{
			return Err(crate::Error::Forbidden);
		}
		if !pending_permitted(&snapshot.bundle, &identity.subject, pending) {
			return Err(crate::Error::Forbidden);
		}
		let draft = crate::apps::registry::workbench::models::AgentDraft::read(
			&mut tx,
			session.draft_id,
			false,
		)
		.await?;
		aidash_application::registry::workbench::authorize(
			&mut crate::bootstrap::draft_authority_scope(&mut tx, &actor, policy),
			&draft.into(),
			"agent_draft.test",
			true,
		)
		.await?;
		let mut scenario = session.scenario;
		scenario["pending_approval"]["state"] = serde_json::json!(match input.choice {
			TestApprovalChoice::AllowOnce => "allow_once",
			TestApprovalChoice::Deny => "deny",
		});
		let mut result = AgentTestSession::complete_listing(&mut tx, id, scenario).await?;
		tx.commit().await?;
		redact(&mut result, None);
		Ok(result)
	}
	pub(crate) async fn get_limits(&self, actor: Actor, id: Uuid) -> Result<TestLimits> {
		Ok(
			aidash_application::registry::workbench::sandbox::get_limits(
				&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
				id,
			)
			.await?
			.into(),
		)
	}
	pub(crate) async fn set_limits(
		&self,
		actor: Actor,
		tenant: String,
		input: TestLimits,
	) -> Result<TenantTestLimits> {
		let repository = crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor);
		let result = aidash_application::registry::workbench::sandbox::set_limits(
			&repository,
			tenant,
			input.into(),
		)
		.await;
		repository
			.tenant_actions
			.record_denial(&self.runtime.store.pool)
			.await?;
		Ok(result?)
	}
	/// A Tenant's limits and the Deployment's ceilings (#361).
	pub(crate) async fn tenant_limits(
		&self,
		actor: Actor,
		tenant: String,
	) -> Result<TenantTestLimits> {
		aidash_domain::policy::identifier(&tenant)?;
		let repository = crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor);
		let result =
			aidash_application::registry::workbench::sandbox::tenant_limits(&repository, tenant)
				.await;
		repository
			.tenant_actions
			.record_denial(&self.runtime.store.pool)
			.await?;
		Ok(result?)
	}
	pub(crate) async fn sessions(&self, actor: Actor, id: Uuid) -> Result<Vec<TestSession>> {
		let mut rows = aidash_application::registry::workbench::sandbox::sessions(
			&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor.clone()),
			id,
		)
		.await?;
		if let Actor::Subject(identity) = &actor
			&& let Some(viewer) = person(&actor)
		{
			let mut tx = self.runtime.store.pool.begin(&actor.tenancy()?).await?;
			let policy = tx.pool().dashboard_policy();
			let snapshot = identity
				.lock_native(&mut tx, false, policy.as_ref())
				.await?;
			for row in &mut rows {
				redact(row, Some((viewer, &identity.subject, &snapshot.bundle)));
			}
			tx.commit().await?;
		} else {
			for row in &mut rows {
				redact(row, None);
			}
		}
		Ok(rows)
	}
	pub(crate) async fn stop(&self, actor: Actor, id: Uuid) -> Result<TestSession> {
		let mut session = aidash_application::registry::workbench::sandbox::stop(
			&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
			id,
		)
		.await?;
		redact(&mut session, None);
		Ok(session)
	}
	pub(crate) async fn start(
		&self,
		actor: Actor,
		id: Uuid,
		input: TestInput,
	) -> Result<TestSession> {
		let runtime = &self.runtime;
		let permit = runtime.sandbox.permit()?;
		let repository = crate::bootstrap::workbench_sandbox_repository(runtime, actor.clone());
		let models = crate::bootstrap::workbench_sandbox_models(runtime);
		let execution = crate::bootstrap::workbench_sandbox_execution(runtime, actor)?;
		let admitted = aidash_application::registry::workbench::sandbox::admission::admit(
			&aidash_application::registry::workbench::sandbox::admission::Admission {
				repository: &repository,
				models: &models,
				plugin_sessions: Some(runtime.store.plugin_sessions.clone()),
			},
			id,
			input,
		)
		.await?;
		let session_id = admitted.session.id;
		let repository = execution.repository.clone();
		let message = admitted.job.input.message.clone();
		if let Err(error) = permit.spawn(
			session_id,
			aidash_runtime::sandbox::complete(execution, session_id, admitted.job),
		) {
			// No job was launched, so persist a known failure and release the admitted active slot.
			aidash_application::registry::workbench::sandbox::execution::settle(
				repository.as_ref(),
				session_id,
				&message,
				Err(
					aidash_application::registry::workbench::sandbox::execution::Failure::Execution(
						aidash_application::Error::Conflict(error.to_string()),
					),
				),
			)
			.await?;
			return Err(error.into());
		}
		let mut session = admitted.session;
		redact(&mut session, None);
		Ok(session)
	}
}

fn person(actor: &Actor) -> Option<Uuid> {
	match actor {
		Actor::Subject(identity) => identity.http_session.as_ref().map(|s| s.identity_id),
		_ => None,
	}
}

fn redact(
	session: &mut TestSession,
	viewer: Option<(Uuid, &str, &aidash_domain::policy::PolicyBundle)>,
) {
	if let Some(scenario) = session.scenario.as_object_mut() {
		let starter = scenario
			.remove("starter_identity")
			.and_then(|value| serde_json::from_value::<Uuid>(value).ok());
		let starter_subject = scenario.remove("starter_subject");
		let permitted = viewer.is_some_and(|(viewer, subject, bundle)| {
			starter == Some(viewer)
				&& starter_subject.as_ref().and_then(|value| value.as_str()) == Some(subject)
				&& scenario.get("pending_approval").is_some_and(|pending| {
					pending["state"] == "pending" && pending_permitted(bundle, subject, pending)
				})
		});
		if !permitted {
			scenario.remove("pending_approval");
		}
	}
}

fn pending_permitted(
	bundle: &aidash_domain::policy::PolicyBundle,
	subject: &str,
	pending: &serde_json::Value,
) -> bool {
	let Some(action) = pending["action"].as_str().filter(|action| {
		matches!(
			*action,
			"tool.call_unapproved" | "network.get" | "research.disclose"
		)
	}) else {
		return false;
	};
	let Ok(resource) =
		serde_json::from_value::<aidash_domain::policy::Resource>(pending["resource"].clone())
	else {
		return false;
	};
	let evaluation = aidash_domain::policy::Evaluation {
		subject: subject.into(),
		action: action.into(),
		resource: resource.clone(),
		environment: serde_json::json!({}),
	};
	let decision =
		aidash_domain::capabilities::approvals::workbench_policy(bundle).evaluate(&evaluation);
	decision.reason == "no_matching_allow"
		&& aidash_domain::capabilities::approvals::workbench_approver_eligible(
			bundle, subject, &resource,
		)
}
