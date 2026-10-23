use super::{journal::Journal, repository};
use crate::apps::execution::models::approval_observations::ApprovalObservation;
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	database::native,
	federation::Federation,
};
use aidash_application::{
	decision::{DecisionGate, observation::*},
	ports::decision::{Approval, DecisionAuthority},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::{
	decision::{observation::*, *},
	registry::Entry,
};
use chrono::{Duration, Utc};
use reinhardt::{
	db::orm::Model,
	query::{
		Alias, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	},
};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) async fn run(runtime: Federation) -> Result<()> {
	loop {
		if let Err(error) = process_one(&runtime).await {
			tracing::warn!(category="approval-observer",error_kind=?std::mem::discriminant(&error),"independent approval observation failed");
		}
		tokio::time::sleep(std::time::Duration::from_millis(750)).await;
	}
}
pub(super) async fn process_one(runtime: &Federation) -> Result<bool> {
	let mut tx = runtime
		.store
		.pool
		.begin(&TenancyScope::Deployment(
			DeploymentScopeReason::ApprovalObservation,
		))
		.await?;
	let row = native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("approval_observations"))
			.and_where(Expr::col("evidence").is_null())
			.and_where(Expr::cust(
				"leased_until IS NULL OR leased_until < CLOCK_TIMESTAMP()",
			))
			.order_by(Alias::new("created_at"), Order::Asc)
			.limit(1)
			.lock(LockType::Update)
			.lock_behavior(reinhardt::query::LockBehavior::SkipLocked)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?;
	let Some(row) = row else {
		tx.commit().await?;
		return Ok(false);
	};
	let id: Uuid = row.try_get("id")?;
	let mut row = ApprovalObservation::objects()
		.filter(ApprovalObservation::field_id().eq(id))
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or_else(|| Error::NotFound("observation".into()))?;
	let lease = ObservationLease {
		worker: Uuid::new_v4(),
		revision: row.lease_revision + 1,
	};
	native::query(
		&Query::update()
			.table(Alias::new("approval_observations"))
			.value(Alias::new("worker"), lease.worker)
			.value(Alias::new("lease_revision"), lease.revision)
			.value(
				Alias::new("leased_until"),
				Utc::now() + Duration::minutes(10),
			)
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	row.worker = Some(lease.worker);
	row.lease_revision = lease.revision;
	let frame: ObservationFrame = serde_json::from_value(row.frame.0.clone())?;
	let journal = Journal {
		store: runtime.store.clone(),
		frame: frame.clone(),
		lease: lease.clone(),
	};
	let authority = Authority {
		runtime: runtime.clone(),
		frame: frame.clone(),
		credential: row.credential_id,
		subject: row.subject.clone(),
	};
	let material = reconstruct(&authority).await;

	let (input, definition) = match material {
		Ok(pair) => (Some(pair.0), Some(pair.1)),
		Err(_) => {
			let mut original = runtime
				.store
				.pool
				.begin(&TenancyScope::tenant(&frame.tenant)?)
				.await?;
			let pin = frame.settings.decider.as_ref().ok_or(Error::Forbidden)?;
			let definition = crate::apps::registry::models::Definition::read_in(
				&mut *original,
				&pin.identity.id,
				&pin.identity.version,
			)
			.await
			.ok();
			original.commit().await?;
			(None, definition)
		}
	};
	let config = frame.settings.config.clone().ok_or(Error::Forbidden)?;
	let provider = aidash_integrations::decision::RouteDecisionProvider::new(
		config,
		aidash_application::registry::validation::decision_implementation_id(),
		Arc::new(
			crate::apps::identity::repositories::provider_credentials::AdmittedAccess {
				store: runtime.store.clone(),
			},
		),
		aidash_application::provider_access::Context {
			tenant: frame.tenant.clone(),
			run: None,
			observation: Some(frame.id),
			..Default::default()
		},
		Arc::new(aidash_integrations::decision::ByteBudget),
		std::time::Duration::from_secs(45),
	)?;
	let gate = DecisionGate {
		authority: &authority,
		journal: &journal,
		provider: &provider,
	};
	gate.observe_approval(
		&ObservationRequest {
			frame: &frame,
			lease: &lease,
			input: input.as_ref(),
			definition: definition.as_ref(),
			now: Utc::now(),
		},
		&journal,
	)
	.await?;
	Ok(true)
}
struct Authority {
	runtime: Federation,
	frame: ObservationFrame,
	credential: Uuid,
	subject: String,
}
impl Authority {
	async fn access(&self) -> Result<Access> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: self.credential,
			tenant: self.frame.tenant.clone(),
			subject: self.subject.clone(),
		};
		let mut access = Access::begin(&self.runtime.store, &identity).await?;
		let workspace = self.workspace(&mut access).await?;
		let node = access.workspace_node(workspace).await?;
		access.set_node(node);
		if let ObservationOrigin::Run { run, .. } = self.frame.origin {
			let current = access.run_for_interaction(run).await?;
			crate::authorization::execution::inherit_run_authority(&mut access, &current).await?;
		}
		Ok(access)
	}
	async fn admitted(&self, access: &mut Access) -> Result<Entry> {
		let pin = self
			.frame
			.settings
			.decider
			.as_ref()
			.ok_or(Error::Forbidden)?;
		let entry =
			crate::authorization::catalog::entry(access, &pin.identity.local(), INVOKE).await?;
		pin.check(&entry)?;
		let workspace_id = self.workspace(access).await?;
		let node = access.workspace_node(workspace_id).await?;
		if !crate::apps::identity::repositories::approval_decider::approved(
			&mut *access.tx,
			&self.frame.tenant,
			&node,
			&pin.identity.local(),
		)
		.await?
		{
			return Err(Error::Forbidden);
		}
		// A live catalog grant never grants private source disclosure.
		let workspace = access.workspace(workspace_id).await?;
		access.require(&workspace, "workspace.read").await?;
		Ok(entry)
	}
	async fn workspace(&self, access: &mut Access) -> Result<Uuid> {
		match self.frame.origin {
			ObservationOrigin::Run { run, .. } => {
				Ok(access.run_for_interaction(run).await?.workspace_id)
			}
			ObservationOrigin::Generation { task, .. } => {
				Ok(access.task_read(task).await?.workspace_id)
			}
		}
	}
}
#[async_trait::async_trait]
impl DecisionAuthority for Authority {
	async fn check(
		&self,
		boundary: &Boundary,
		pin: &DeciderPin,
		sources: &[SourcePin],
	) -> aidash_application::Result<Approval> {
		if !sources.is_empty()
			|| boundary.run != self.frame.id
			|| Some(pin) != self.frame.settings.decider.as_ref()
		{
			return Err(aidash_application::Error::Forbidden);
		}
		reconstruct(self).await?;
		Ok(Approval {
			restrictions: self.frame.settings.restrictions.clone(),
			state_retention: Default::default(),
		})
	}
	async fn read(&self, _: &Evidence, _: bool) -> aidash_application::Result<()> {
		Err(aidash_application::Error::Forbidden)
	}
}
async fn reconstruct(authority: &Authority) -> Result<(ObservationInput, Entry)> {
	let frame = &authority.frame;
	let mut access = authority.access().await?;
	let result = async {
		let definition = authority.admitted(&mut access).await?;
		let mut input = ObservationInput {
			inputs: vec![],
			instructions: None,
			policies: vec![],
			prior_calls: vec![],
			pending: match &frame.origin {
				ObservationOrigin::Run { .. } => {
					let mut found = None;
					for kind in aidash_application::capabilities::approvals::KINDS {
						match crate::apps::execution::capabilities::services::records::get(
							&mut access,
							frame.approval,
							kind,
						)
						.await
						{
							Ok(r) => {
								found = Some(repository::pending(&mut *access.tx, &r).await?);
								break;
							}
							Err(Error::NotFound(_)) => {}
							Err(e) => return Err(e),
						}
					}
					found.ok_or_else(|| Error::NotFound("original approval action".into()))?
				}
				ObservationOrigin::Generation { request, .. } => {
					let job = crate::apps::execution::generation::repositories::lifecycle::load(
						&mut access.tx,
						&frame.tenant,
						*request,
					)
					.await?;
					repository::generation_action(&job)
				}
			},
		};
		match frame.origin {
			ObservationOrigin::Run { run, .. } => {
				let inputs =
					crate::apps::execution::models::RunInput::for_run(&mut *access.tx, run).await?;
				for reference in &frame.input_refs {
					let i = inputs
						.iter()
						.find(|i| reference.resource == format!("input:{run}:{}", i.seq))
						.ok_or_else(|| Error::NotFound("original human input".into()))?;
					if !access
						.snapshot
						.bundle
						.subjects
						.get(&i.sender)
						.is_some_and(|s| {
							s.enabled && s.kind == aidash_domain::policy::SubjectKind::User
						}) {
						return Err(Error::Forbidden);
					}
					input.inputs.push(InputMaterial {
						reference: reference.clone(),
						sequence: Some(i.seq),
						content: json!(i.content),
					});
				}
				if let Some(reference) = &frame.instruction_ref {
					let raw = reference
						.resource
						.strip_prefix("agent:")
						.and_then(|r| r.rsplit_once('@'))
						.ok_or(Error::Forbidden)?;
					let entry = crate::apps::registry::models::Definition::read_in(
						&mut *access.tx,
						raw.0,
						raw.1,
					)
					.await?;
					input.instructions = Some(ReferenceMaterial {
						reference: reference.clone(),
						content: entry
							.config
							.get("instructions")
							.cloned()
							.unwrap_or(json!("")),
					});
				}
				for reference in &frame.prior_context_refs {
					let seq: i64 = reference
						.resource
						.rsplit(':')
						.next()
						.ok_or(Error::Forbidden)?
						.parse()
						.map_err(|_| Error::Forbidden)?;
					let value: Value = native::query_scalar(
						&Query::select()
							.column(Alias::new("event"))
							.from(Alias::new("run_context_events"))
							.and_where(Expr::col("run_id").eq(Expr::value(run)))
							.and_where(Expr::col("seq").eq(seq))
							.to_string(PostgresQueryBuilder),
					)
					.scalar_one(&mut *access.tx)
					.await?;
					let call = &value["call"];
					let name = call["name"].as_str().ok_or(Error::Forbidden)?.to_owned();
					let arguments = call["arguments"].clone();
					input.prior_calls.push(PriorCallMaterial {
						reference: reference.clone(),
						name,
						arguments,
					});
				}
				for reference in &frame.policy_refs {
					if reference.resource != format!("policy:{}", access.snapshot.revision) {
						return Err(Error::NotFound("original policy revision".into()));
					}
					input.policies.push(ReferenceMaterial {
						reference: reference.clone(),
						content: json!({"authorization_revision":access.snapshot.revision,"effective_restrictions":frame.settings.restrictions}),
					});
				}
			}
			ObservationOrigin::Generation {
				task,
				ref policy_id,
				policy_revision,
				..
			} => {
				let task = access.task_read(task).await?;
				let seed = json!({"title":task.title,"description":task.description});
				for reference in &frame.input_refs {
					input.inputs.push(InputMaterial {
						reference: reference.clone(),
						sequence: None,
						content: seed.clone(),
					});
				}
				for reference in &frame.policy_refs {
					let spec: Value = native::query_scalar(
						&Query::select()
							.column(Alias::new("spec"))
							.from(Alias::new("generation_policy_history"))
							.and_where(Expr::col("tenant").eq(frame.tenant.clone()))
							.and_where(Expr::col("policy_id").eq(policy_id.clone()))
							.and_where(Expr::col("revision").eq(policy_revision))
							.to_string(PostgresQueryBuilder),
					)
					.scalar_one(&mut *access.tx)
					.await?;
					input.policies.push(ReferenceMaterial {
						reference: reference.clone(),
						content: spec,
					});
				}
			}
		}
		input.validate(frame)?;
		Ok((input, definition))
	}
	.await;
	access.finish(result).await
}

/// Native provider scope is proved by the durable observer job and attempt, never a caller's UUID alone.
pub(crate) async fn provider_scope(
	store: &crate::store::Store,
	context: &aidash_application::provider_access::Context,
	reserved: bool,
) -> Result<()> {
	let Some(observation) = context.observation else {
		return Ok(());
	};
	if context.run.is_some() || context.maintenance.is_some() || !context.is_decision() {
		return Err(Error::Forbidden);
	}
	let call = context
		.inference
		.as_ref()
		.and_then(|i| i.call)
		.ok_or(Error::Forbidden)?;
	let mut tx = store
		.control_pool
		.begin(&TenancyScope::tenant(&context.tenant)?)
		.await?;
	let job = ApprovalObservation::objects()
		.filter(ApprovalObservation::field_id().eq(observation))
		.filter(ApprovalObservation::field_tenant().eq(context.tenant.clone()))
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	if job.evidence.is_some()
		|| job.worker.is_none()
		|| job.leased_until.is_none_or(|until| until <= Utc::now())
	{
		return Err(Error::Forbidden);
	}
	let frame: ObservationFrame = serde_json::from_value(job.frame.0)?;

	frame.validate()?;
	let inference = context.inference.as_ref().ok_or(Error::Forbidden)?;
	let config = frame.settings.config.as_ref().ok_or(Error::Forbidden)?;
	if inference.operations != [aidash_application::provider_access::Operation::Decision]
		|| inference.max_output_tokens != 0
		|| inference.cache_breakpoints
		|| !config
			.routes
			.iter()
			.any(|route| route.model == inference.model && route.hosts == inference.hosts)
	{
		return Err(Error::Forbidden);
	}
	if reserved {
		let attempt = native::query(
			&Query::select()
				.columns(["record", "status"].map(Alias::new))
				.from(Alias::new("approval_observation_attempts"))
				.and_where(Expr::col("id").eq(Expr::value(call)))
				.and_where(Expr::col("observation").eq(Expr::value(observation)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or(Error::Forbidden)?;
		let record: Value = attempt.try_get("record")?;
		let status: String = attempt.try_get("status")?;
		let expected = observation_boundary(
			&frame,
			&ObservationLease {
				worker: job.worker.ok_or(Error::Forbidden)?,
				revision: job.lease_revision,
			},
		)?;
		let route = record["route"]
			.as_u64()
			.and_then(|r| config.routes.get(r as usize))
			.ok_or(Error::Forbidden)?;
		if route.model != inference.model || route.hosts != inference.hosts {
			return Err(Error::Forbidden);
		}
		if status != "pending"
			|| record["boundary"] != json!(expected)
			|| record["decider"] != json!(frame.settings.decider)
			|| record["attempt"] != json!(call)
		{
			return Err(Error::Forbidden);
		}
	}
	tx.commit().await?;
	// Current subject, workspace disclosure and exact Catalog Approval remain independent checks.
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: job.credential_id,
		tenant: context.tenant.clone(),
		subject: job.subject,
	};
	let mut access = Access::begin(store, &identity).await?;
	let result = async {
		let node = access.workspace_node(job.workspace_id).await?;
		access.set_node(node.clone());
		let workspace = access.workspace(job.workspace_id).await?;
		access.require(&workspace, "workspace.read").await?;
		let pin = frame.settings.decider.as_ref().ok_or(Error::Forbidden)?;
		let entry =
			crate::authorization::catalog::entry(&mut access, &pin.identity.local(), INVOKE)
				.await?;
		pin.check(&entry)?;
		if !crate::apps::identity::repositories::approval_decider::approved(
			&mut *access.tx,
			&frame.tenant,
			&node,
			&pin.identity.local(),
		)
		.await?
		{
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	.await;
	access.finish(result).await
}
