use super::super::models::approval_observations::{ApprovalObservation, ObserverSetting};
use crate::{Error, Result, authorization::access::Access};
use aidash_domain::{
	decision::{observation::*, *},
	registry::{EntityRef, bindings::QualifiedRef, rules::digest},
};
use chrono::Utc;
use reinhardt::{
	db::{
		backends::TransactionExecutor,
		orm::{Model, execution::convert_values},
	},
	query::{
		Alias, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
	},
};
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) async fn settings(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<ObserverSettings> {
	let row = ObserverSetting::objects()
		.filter(ObserverSetting::field_tenant().eq(tenant))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop();
	let setting = row
		.map(|r| serde_json::from_value(r.settings.0))
		.transpose()?
		.unwrap_or_default();
	Ok(setting)
}
pub(super) async fn save_settings(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	expected: i64,
	target: Option<&EntityRef>,
	node: &aidash_domain::nodes::NodeName,
	deployment: &str,
) -> Result<ObserverSettings> {
	crate::apps::identity::repositories::approval_decider::lock(tx, tenant).await?;
	let old = settings(tx, tenant).await?;
	if old.revision != expected {
		return Err(Error::Conflict("observer setting revision changed".into()));
	}
	let mut selected = ObserverSettings {
		revision: expected
			.checked_add(1)
			.ok_or_else(|| Error::Invalid("setting revision exhausted".into()))?,
		..Default::default()
	};
	if let Some(target) = target {
		let entry =
			crate::apps::registry::models::Definition::read_in(tx, &target.id, &target.version)
				.await?;
		let config: DeciderConfig = serde_json::from_value(entry.config.clone())?;
		if entry.kind != "decider"
			|| config.hook != Hook::ApprovalObservation
			|| config.mode != Mode::Shadow
		{
			return Err(Error::Invalid(
				"observer requires a Shadow approval_observation Decider".into(),
			));
		}
		if !crate::apps::identity::repositories::approval_decider::approved(
			tx, tenant, node, target,
		)
		.await?
		{
			return Err(Error::Invalid(
				"observer exact version requires Catalog Approval".into(),
			));
		}
		selected.decider = Some(DeciderPin {
			identity: QualifiedRef {
				registry_deployment: deployment.into(),
				id: entry.id.clone(),
				version: entry.version.clone(),
			},
			definition_digest: digest(&json!(entry)),
			configuration_digest: config.digest()?,
			provider_implementation:
				aidash_application::registry::validation::decision_implementation_id(),
		});
		selected.config = Some(config);
	}
	selected.validate()?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new("approval_observer_settings"))
		.columns(["tenant", "revision", "settings", "updated_at"].map(Alias::new))
		.values_panic::<_, reinhardt::query::Value>([
			tenant.into(),
			selected.revision.into(),
			json!(selected).into(),
			Utc::now().into(),
		])
		.on_conflict(
			OnConflict::column(Alias::new("tenant"))
				.update_columns(["revision", "settings", "updated_at"].map(Alias::new))
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(selected)
}
pub(super) fn reference(resource: String, value: &Value) -> FrameReference {
	FrameReference {
		resource,
		revision_digest: digest(value),
	}
}
fn human(access: &Access, sender: &str) -> bool {
	access
		.snapshot
		.bundle
		.subjects
		.get(sender)
		.is_some_and(|s| s.enabled && s.kind == aidash_domain::policy::SubjectKind::User)
}
pub(super) async fn pending(
	tx: &mut dyn TransactionExecutor,
	record: &crate::apps::execution::capabilities::serializers::records::Record,
) -> Result<PendingAction> {
	let data = &record.data;
	Ok(match record.kind.as_str() {
		"tool_call" => {
			let id = data["tool"]
				.as_str()
				.ok_or_else(|| Error::Invalid("missing Tool reference".into()))?;
			let version = data["version"]
				.as_str()
				.ok_or_else(|| Error::Invalid("missing Tool version".into()))?;
			let definition =
				crate::apps::registry::models::Definition::read_in(tx, id, version).await?;
			let call = &data["call"];
			PendingAction::ToolCall {
				name: call["name"].as_str().unwrap_or(id).into(),
				arguments: call["arguments"].clone(),
				definition: json!(definition),
			}
		}
		"outbound" => PendingAction::Outbound {
			name: "network.get".into(),
			arguments: json!({"url":data["url"]}),
			definition: json!({"operation":"network.get","method":"GET","targets":data["targets"]}),
		},
		"disclosure" if data["operation"] == "search" => PendingAction::WebSearch {
			name: "web.search".into(),
			arguments: json!({"query":data["query"],"content":data["content"]}),
			definition: json!({"operation":data["operation"],"provider":data["provider"],"destination":data["destination"],"attribution":data["attribution"]}),
		},
		"disclosure" => PendingAction::PageReading {
			name: "web.read".into(),
			arguments: json!({"content":data["content"],"query":data["query"]}),
			definition: json!({"operation":data["operation"],"provider":data["provider"],"destination":data["destination"],"attribution":data["attribution"]}),
		},
		_ => return Err(Error::Invalid("unsupported observed approval kind".into())),
	})
}
pub(super) fn initial_settlement(state: &str, data: &Value) -> SettlementClass {
	let by = if data["given_by"].is_object() {
		&data["given_by"]
	} else {
		&data["refused_by"]
	};
	if by.get("rule").is_some() {
		SettlementClass::Rule
	} else if by["mode"] == "auto" {
		SettlementClass::Auto
	} else if by["mode"] == "bypass" {
		SettlementClass::Bypass
	} else if matches!(
		state,
		"approved" | "consumed" | "denied" | "blocked" | "attempted"
	) {
		SettlementClass::Auto
	} else {
		SettlementClass::Pending
	}
}
pub(crate) async fn enqueue_capability(
	access: &mut Access,
	record: &crate::apps::execution::capabilities::serializers::records::Record,
	original: Option<&aidash_domain::Run>,
) -> Result<()> {
	if !aidash_application::capabilities::approvals::KINDS.contains(&record.kind.as_str()) {
		return Ok(());
	}
	let created_at = Utc::now();
	let original_run: Uuid = serde_json::from_value(record.data["run_id"].clone())?;
	population(
		&mut *access.tx,
		&record.tenant,
		record.id,
		"capability",
		capability_kind(record),
		Some(original_run),
		initial_settlement(&record.state, &record.data),
		created_at,
	)
	.await?;
	let selected = settings(&mut *access.tx, &record.tenant).await?;
	if selected.decider.is_none() {
		return Ok(());
	}

	access.tx.savepoint("approval_observer_capture").await?;
	match capture_capability(access, record, original, selected.clone(), created_at).await {
		Ok(()) => access
			.tx
			.release_savepoint("approval_observer_capture")
			.await
			.map_err(Into::into),
		Err(_) => {
			access
				.tx
				.rollback_to_savepoint("approval_observer_capture")
				.await?;
			access
				.tx
				.release_savepoint("approval_observer_capture")
				.await?;
			unavailable(
				&mut *access.tx,
				&record.tenant,
				record.id,
				capability_kind(record),
				&selected,
			)
			.await
		}
	}
}
fn capability_kind(
	record: &crate::apps::execution::capabilities::serializers::records::Record,
) -> ApprovalKind {
	match record.kind.as_str() {
		"tool_call" => ApprovalKind::ToolCall,
		"outbound" => ApprovalKind::Outbound,
		_ if record.data["operation"] == "search" => ApprovalKind::WebSearch,
		_ => ApprovalKind::PageReading,
	}
}
async fn capture_capability(
	access: &mut Access,
	record: &crate::apps::execution::capabilities::serializers::records::Record,
	original: Option<&aidash_domain::Run>,
	mut selected: ObserverSettings,
	created_at: chrono::DateTime<Utc>,
) -> Result<()> {
	let run_id: Uuid = serde_json::from_value(record.data["run_id"].clone())?;
	let run: aidash_domain::Run = crate::database::query_as(
		&Query::select()
			.expr(Expr::col(reinhardt::query::ColumnRef::Asterisk))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run_id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *access.tx)
	.await?;
	let boundary = original
		.filter(|original| original.id == run.id)
		.unwrap_or(&run);
	if let Some(snapshot) = run.context.binding_snapshot.as_ref()
		&& let Some((bound, _)) = snapshot.bound_decider(Hook::Approval)?
	{
		let config = selected.config.as_ref().ok_or(Error::Forbidden)?;
		selected.restrictions.threshold =
			if bound.restrictions.threshold.value() < selected.restrictions.threshold.value() {
				bound.restrictions.threshold
			} else {
				selected.restrictions.threshold
			};
		selected.restrictions.max_calls_per_run = bound
			.restrictions
			.max_calls_per_run
			.map(|limit| limit.min(config.max_calls_per_run));
	}
	let inputs = crate::apps::execution::models::RunInput::for_run(&mut *access.tx, run_id).await?;
	let input_refs = inputs
		.iter()
		.filter(|i| i.seq <= boundary.observed_input_seq && human(access, &i.sender))
		.map(|i| reference(format!("input:{run_id}:{}", i.seq), &json!(i.content)))
		.collect();
	let definition = crate::apps::registry::models::Definition::read_in(
		&mut *access.tx,
		&run.agent_id,
		&run.agent_version,
	)
	.await?;
	let instructions = definition
		.config
		.get("instructions")
		.cloned()
		.unwrap_or(json!(""));
	let action = pending(&mut *access.tx, record).await;
	// An unavailable immutable definition is retained as an explicit outcome, without a call.
	let (kind, pending_digest) = match action {
		Ok(ref action) => (action.kind(), action.digest()?),
		Err(_) => (
			match record.kind.as_str() {
				"tool_call" => ApprovalKind::ToolCall,
				"outbound" => ApprovalKind::Outbound,
				_ if record.data["operation"] == "search" => ApprovalKind::WebSearch,
				_ => ApprovalKind::PageReading,
			},
			digest(&record.data),
		),
	};

	// Capture references from the actual worker boundary when it is available.
	// Reconstruction may only resolve these exact journal sequences, never future history.
	let mut prior_context_refs = vec![];
	if let Some(original) = original.filter(|original| original.id == run.id) {
		for entry in &original.context.history {
			if let aidash_domain::context::ContextEvent::Tool { call, .. } = &entry.event {
				if record.data["call"]["id"] == call.id {
					continue;
				}
				prior_context_refs.push(reference(
					format!("context:{}:{}", run.id, entry.seq),
					&json!({"name":call.name,"arguments":call.arguments}),
				));
			}
		}
	} else {
		let prior = crate::database::native::query(
			&Query::select()
				.columns(["seq", "event"].map(Alias::new))
				.from(Alias::new("run_context_events"))
				.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
				.order_by(Alias::new("seq"), reinhardt::query::Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&mut *access.tx)
		.await?;
		for row in prior {
			let seq: i64 = row.try_get("seq")?;
			let event: Value = row.try_get("event")?;
			if let Some(call) = event.get("call") {
				prior_context_refs.push(reference(
					format!("context:{}:{seq}", run.id),
					&json!({"name":call["name"],"arguments":call["arguments"]}),
				));
			}
		}
	}
	let policy_material = json!({"authorization_revision":access.snapshot.revision,"effective_restrictions":selected.restrictions});
	let mut frame = ObservationFrame {
		version: 1,
		id: Uuid::nil(),
		tenant: record.tenant.clone(),
		approval: record.id,
		node: selected
			.decider
			.as_ref()
			.ok_or(Error::Forbidden)?
			.identity
			.registry_deployment
			.clone(),
		created_at,
		kind,
		origin: ObservationOrigin::Run {
			run: run.id,
			step: boundary.step,
			input_revision: boundary.observed_input_seq,
			cycle: boundary.recovery.decision_cycle,
		},
		pending_digest: pending_digest.clone(),
		settings: selected,
		input_cutoff: boundary.observed_input_seq,
		input_refs,
		instruction_ref: Some(reference(
			format!("agent:{}@{}", run.agent_id, run.agent_version),
			&instructions,
		)),
		action_ref: FrameReference {
			resource: format!("capability:{}", record.id),
			revision_digest: pending_digest.clone(),
		},
		policy_refs: vec![reference(
			format!("policy:{}", access.snapshot.revision),
			&policy_material,
		)],
		prior_context_refs,
		operation_class: record.data["operation"].as_str().map(str::to_owned),
		effect_class: Some(record.data["action"].as_str().unwrap_or("unknown").into()),
	};
	frame.id = frame.identity();
	insert(
		access,
		frame,
		"capability",
		Some(run.id),
		run.workspace_id,
		initial_settlement(&record.state, &record.data),
	)
	.await
}
pub(super) fn generation_action(
	job: &aidash_domain::generation::requests::Request,
) -> PendingAction {
	PendingAction::Generation {
		request: PendingGeneration {
			request: job.id,
			task: job.task_id,
			policy_id: job.policy_id.clone(),
			policy_revision: job.policy_revision,
			requested_agent_id: job.agent_id.clone(),
			requested_agent_version: job.agent_version.clone(),
			definition: job.definition.clone(),
			token_limit: job.token_limit,
			depth: job.depth,
		},
	}
}
pub(crate) async fn enqueue_generation(
	access: &mut Access,
	job: &aidash_domain::generation::requests::Request,
	given_by: Option<aidash_domain::approval_rules::GivenBy>,
) -> Result<()> {
	let class = match given_by {
		Some(aidash_domain::approval_rules::GivenBy::Rule(_)) => SettlementClass::Rule,
		Some(aidash_domain::approval_rules::GivenBy::Bypass) => SettlementClass::Bypass,
		_ if job.status == "PENDING_APPROVAL" => SettlementClass::Pending,
		_ => SettlementClass::Auto,
	};
	population(
		&mut *access.tx,
		&job.tenant,
		job.id,
		"generation",
		ApprovalKind::Generation,
		None,
		class,
		job.created_at,
	)
	.await?;
	let selected = settings(&mut *access.tx, &job.tenant).await?;
	if selected.decider.is_none() {
		return Ok(());
	}

	access.tx.savepoint("approval_observer_capture").await?;
	match capture_generation(access, job, selected.clone(), class).await {
		Ok(()) => access
			.tx
			.release_savepoint("approval_observer_capture")
			.await
			.map_err(Into::into),
		Err(_) => {
			access
				.tx
				.rollback_to_savepoint("approval_observer_capture")
				.await?;
			access
				.tx
				.release_savepoint("approval_observer_capture")
				.await?;
			unavailable(
				&mut *access.tx,
				&job.tenant,
				job.id,
				ApprovalKind::Generation,
				&selected,
			)
			.await
		}
	}
}
async fn capture_generation(
	access: &mut Access,
	job: &aidash_domain::generation::requests::Request,
	selected: ObserverSettings,
	class: SettlementClass,
) -> Result<()> {
	let task = access.task_read(job.task_id).await?;
	let seed = json!({"title":task.title,"description":task.description});
	let action = generation_action(job);
	let policy: Value = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("spec"))
			.from(Alias::new("generation_policy_history"))
			.and_where(Expr::col("tenant").eq(job.tenant.clone()))
			.and_where(Expr::col("policy_id").eq(job.policy_id.clone()))
			.and_where(Expr::col("revision").eq(job.policy_revision))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *access.tx)
	.await?;
	let mut frame = ObservationFrame {
		version: 1,
		id: Uuid::nil(),
		tenant: job.tenant.clone(),
		approval: job.id,
		node: selected
			.decider
			.as_ref()
			.ok_or(Error::Forbidden)?
			.identity
			.registry_deployment
			.clone(),
		created_at: job.created_at,
		kind: ApprovalKind::Generation,
		origin: ObservationOrigin::Generation {
			request: job.id,
			task: job.task_id,
			policy_id: job.policy_id.clone(),
			policy_revision: job.policy_revision,
		},
		pending_digest: action.digest()?,
		settings: selected,
		input_cutoff: 0,
		input_refs: vec![reference(format!("task:{}", job.task_id), &seed)],
		instruction_ref: None,
		action_ref: reference(format!("generation:{}", job.id), &json!(action)),
		policy_refs: vec![reference(
			format!(
				"generation-policy:{}:{}",
				job.policy_id, job.policy_revision
			),
			&policy,
		)],
		prior_context_refs: vec![],
		operation_class: None,
		effect_class: Some("generation".into()),
	};
	frame.id = frame.identity();
	insert(access, frame, "generation", None, job.workspace_id, class).await
}
async fn insert(
	access: &mut Access,
	frame: ObservationFrame,
	approval_type: &str,
	run: Option<Uuid>,
	workspace: Uuid,
	class: SettlementClass,
) -> Result<()> {
	frame.validate()?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new("approval_observations"))
		.columns(
			[
				"id",
				"tenant",
				"approval",
				"approval_type",
				"run_id",
				"workspace_id",
				"credential_id",
				"subject",
				"frame",
				"settlement",
				"lease_revision",
				"created_at",
			]
			.map(Alias::new),
		)
		.values_panic::<_, reinhardt::query::Value>([
			frame.id.into(),
			frame.tenant.clone().into(),
			frame.approval.into(),
			approval_type.into(),
			reinhardt::query::IntoValue::into_value(run),
			workspace.into(),
			access.identity.credential_id.into(),
			access.identity.subject.clone().into(),
			json!(frame).into(),
			serde_json::to_value(class)?.as_str().unwrap().into(),
			0i64.into(),
			frame.created_at.into(),
		])
		.on_conflict(
			OnConflict::columns(["tenant", "approval_type", "approval"].map(Alias::new))
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	access.tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}
pub(crate) async fn label(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	approval: Uuid,
	human: HumanLabel,
) -> Result<()> {
	settlement(
		tx,
		tenant,
		approval,
		match human {
			HumanLabel::Allow => SettlementClass::HumanAllow,
			HumanLabel::Deny => SettlementClass::HumanDeny,
		},
	)
	.await
}
pub(crate) async fn settlement(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	approval: Uuid,
	class: SettlementClass,
) -> Result<()> {
	let (sql, values) = Query::update()
		.table(Alias::new("approval_observations"))
		.value(
			Alias::new("settlement"),
			serde_json::to_value(class)?.as_str().unwrap(),
		)
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("approval").eq(Expr::value(approval)))
		.and_where(Expr::col("settlement").is_not_in(["human_allow", "human_deny"]))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	let (sql, values) = Query::update()
		.table(Alias::new("approval_observation_population"))
		.value(
			Alias::new("settlement"),
			serde_json::to_value(class)?.as_str().unwrap(),
		)
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("approval").eq(Expr::value(approval)))
		.and_where(Expr::col("settlement").is_not_in(["human_allow", "human_deny"]))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}
pub(super) async fn observation(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	approval: Uuid,
) -> Result<Option<ApprovalObservation>> {
	Ok(ApprovalObservation::objects()
		.filter(ApprovalObservation::field_tenant().eq(tenant))
		.filter(ApprovalObservation::field_approval().eq(approval))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop())
}

async fn population(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	approval: Uuid,
	approval_type: &str,
	kind: ApprovalKind,
	run: Option<Uuid>,
	settled: SettlementClass,
	created: chrono::DateTime<Utc>,
) -> Result<()> {
	let (sql, values) = Query::insert()
		.into_table(Alias::new("approval_observation_population"))
		.columns(
			[
				"tenant",
				"approval",
				"approval_type",
				"kind",
				"run_id",
				"settlement",
				"created_at",
			]
			.map(Alias::new),
		)
		.values_panic::<_, reinhardt::query::Value>([
			tenant.into(),
			approval.into(),
			approval_type.into(),
			serde_json::to_value(kind)?.as_str().unwrap().into(),
			reinhardt::query::IntoValue::into_value(run),
			serde_json::to_value(settled)?.as_str().unwrap().into(),
			created.into(),
		])
		.on_conflict(
			OnConflict::column(Alias::new("approval"))
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

async fn unavailable(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	approval: Uuid,
	kind: ApprovalKind,
	selected: &ObserverSettings,
) -> Result<()> {
	let pin = selected.decider.as_ref().ok_or(Error::Forbidden)?;
	let view = json!({"kind":kind,"settings_revision":selected.revision,"decider":pin.identity,"configuration_digest":pin.configuration_digest,"definition_digest":pin.definition_digest,"outcome":{"status":"unavailable","reason":"missing_state"},"routes":[],"human_label":null});
	let (sql, values) = Query::update()
		.table(Alias::new("approval_observation_population"))
		.value(Alias::new("observer_unavailable"), view)
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("approval").eq(Expr::value(approval)))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}
