//! Explicit Home generation intents and prepared foreign-task executors. No
//! local Task or Run is fabricated to reuse local generation privileges.
use super::{Assignment, Request, policy, remote::Ancestor};
use crate::{
	Error, Result,
	authorization::{
		access::Access,
		identity::{Actor, SubjectIdentity},
		peer,
		policy::{Subject, SubjectKind},
	},
	domain::Task,
	federation::Federation,
	registry::EntityRef,
};
use axum::{
	Extension, Json,
	extract::{Path, State},
	http::HeaderMap,
};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{
	Alias, Asterisk, Expr, LockType, OnConflict, PostgresQueryBuilder, Query,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as=RemoteGenerationIntent)]
pub(crate) struct Intent {
	pub id: Uuid,
	pub home_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub task: Task,
	pub target_node: String,
	pub policy_id: String,
	pub policy_revision: i64,
	pub lineage: Vec<Ancestor>,
	pub reason: String,
	pub ttl_seconds: i64,
	pub expires_at: DateTime<Utc>,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as=RemoteGenerationInput)]
pub(crate) struct Input {
	id: Uuid,
	node_id: String,
	policy_id: String,
	policy_revision: i64,
	ttl_seconds: i64,
	reason: String,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as=RemoteGenerationPrepared)]
pub(crate) struct Prepared {
	intent_id: Uuid,
	node_id: String,
	request_id: Uuid,
	agent: EntityRef,
	status: String,
	prepared: bool,
	expires_at: DateTime<Utc>,
}
#[derive(sqlx::FromRow)]
struct Record {
	tenant: String,
	credential_id: Uuid,
	root_subject: String,
	subject_chain: Vec<String>,
	binding: Value,
	cancelled: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reference {
	pub intent_id: Uuid,
}
async fn load(f: &Federation, id: Uuid) -> Result<Record> {
	sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_remote_intents"))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_optional(&f.store.pool)
	.await?
	.ok_or(Error::Forbidden)
}
async fn home_authority(
	access: &mut Access,
	task: &Task,
	target: &str,
	policy_id: &str,
) -> Result<()> {
	let workspace = access.workspace(task.workspace_id).await?;
	access.require(&workspace, "workspace.read").await?;
	access.require(&workspace, "generation.disclose").await?;
	let resource = access.task_resource(task).await?;
	access.require(&resource, "task.read").await?;
	access.require(&resource, "task.delegate").await?;
	access
		.require(
			&access.resource("node", target, json!({})),
			"federation.execute",
		)
		.await?;
	access
		.require(
			&access.resource(
				"generation_policy",
				format!("{target}/generation-policies/{policy_id}"),
				json!({"remote_node":target}),
			),
			"generation.request",
		)
		.await?;
	Ok(())
}
async fn home_lease(
	f: &Federation,
	source: &str,
	id: Uuid,
	exclusive: bool,
) -> Result<(Access, Intent)> {
	let record = load(f, id).await?;
	let intent: Intent = serde_json::from_value(record.binding.clone())?;
	if record.cancelled
		|| intent.target_node != source
		|| intent.expires_at <= Utc::now()
		|| intent.home_node != f.config.node_id
	{
		return Err(Error::Forbidden);
	}
	let identity = SubjectIdentity {
		credential_id: record.credential_id,
		tenant: record.tenant.clone(),
		subject: record.root_subject.clone(),
	};
	let mut access = if exclusive {
		Access::begin_exclusive(&f.store, &identity).await?
	} else {
		Access::begin(&f.store, &identity).await?
	};
	let result: Result<()> = async {
		let current: Record = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("generation_remote_intents"))
				.and_where(Expr::cust("id=$1"))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_one(&mut **access.tx)
		.await?;
		if current.cancelled
			|| current.binding != record.binding
			|| current.subject_chain != record.subject_chain
			|| current.credential_id != record.credential_id
		{
			return Err(Error::Forbidden);
		}
		access.subjects = record.subject_chain;
		let task = access.task_read(intent.task.id).await?;
		if task.revision != intent.task.revision || task.status != "OPEN" {
			return Err(Error::Forbidden);
		}
		home_authority(&mut access, &task, source, &intent.policy_id).await?;
		if super::remote::lineage(&mut access, &f.config.node_id).await? != intent.lineage {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	.await;
	if let Err(error) = result {
		return access.finish(Err(error)).await;
	}
	Ok((access, intent))
}
#[utoipa::path(post,path="/tasks/{id}/remote-generation",operation_id="remote_generation_prepare",params(("id"=Uuid,Path)),request_body=Input,responses((status=200,body=Prepared)),security(("bearer_auth"=[])))]
pub(crate) async fn request(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(task_id): Path<Uuid>,
	Json(input): Json<Input>,
) -> Result<Json<Prepared>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	crate::config::validate_node_id(&input.node_id)?;
	if input.id.is_nil()
		|| input.node_id == f.config.node_id
		|| input.policy_revision < 1
		|| !(1..=3600).contains(&input.ttl_seconds)
		|| input.reason.trim().is_empty()
		|| input.reason.len() > 4096
	{
		return Err(Error::Invalid("invalid remote generation intent".into()));
	}
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		crate::authorization::execution::inherit_task_origin(&mut access, task_id).await?;
		let task = access.task_read(task_id).await?;
		if task.status != "OPEN" {
			return Err(Error::Conflict("task is already assigned".into()));
		}
		home_authority(&mut access, &task, &input.node_id, &input.policy_id).await?;
		let intent = Intent {
			id: input.id,
			home_node: f.config.node_id.clone(),
			source_tenant: identity.tenant.clone(),
			source_subject: identity.subject.clone(),
			task,
			target_node: input.node_id.clone(),
			policy_id: input.policy_id.clone(),
			policy_revision: input.policy_revision,
			lineage: super::remote::lineage(&mut access, &f.config.node_id).await?,
			reason: input.reason.clone(),
			ttl_seconds: input.ttl_seconds,
			expires_at: Utc::now() + chrono::Duration::seconds(input.ttl_seconds),
		};
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("generation_remote_intents"))
				.columns(
					[
						"id",
						"task_id",
						"tenant",
						"credential_id",
						"root_subject",
						"subject_chain",
						"binding",
					]
					.map(Alias::new),
				)
				.values_panic(["$1", "$2", "$3", "$4", "$5", "$6", "$7"].map(Expr::cust))
				.on_conflict(OnConflict::new().do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.bind(input.id)
		.bind(task_id)
		.bind(&identity.tenant)
		.bind(identity.credential_id)
		.bind(&identity.subject)
		.bind(&access.subjects)
		.bind(json!(intent))
		.execute(&mut **access.tx)
		.await?;
		let saved: Record = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("generation_remote_intents"))
				.and_where(Expr::cust("id=$1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(input.id)
		.fetch_one(&mut **access.tx)
		.await?;
		let old: Intent = serde_json::from_value(saved.binding)?;
		let mut expected = intent;
		expected.expires_at = old.expires_at;
		if saved.cancelled
			|| saved.tenant != identity.tenant
			|| saved.credential_id != identity.credential_id
			|| saved.subject_chain != access.subjects
			|| json!(expected) != json!(old)
		{
			return Err(Error::Conflict(
				"generation intent already binds different authority or policy".into(),
			));
		}
		Ok(())
	}
	.await;
	access.finish(result).await?;
	let prepared: Prepared = peer::authority_request(
		&f,
		&input.node_id,
		"/scoped/generation/prepare",
		&json!({"intent_id":input.id}),
	)
	.await?;
	if prepared.intent_id != input.id || prepared.node_id != input.node_id {
		return Err(Error::Forbidden);
	}
	if prepared.prepared {
		let (mut access, intent) = home_lease(&f, &input.node_id, input.id, true).await?;
		let result = async {
			let subject = crate::domain::qualified_agent(
				&input.node_id,
				&prepared.agent.id,
				&prepared.agent.version,
			);
			let value = Subject {
				kind: SubjectKind::Agent,
				roles: Default::default(),
				groups: Default::default(),
				attributes: json!({"generation_intent":intent.id,"remote_node":input.node_id}),
				enabled: true,
				delegated_by: access.subjects.last().cloned(),
			};
			if let Some(old) = access.snapshot.bundle.subjects.get(&subject) {
				if json!(old) != json!(value) {
					return Err(Error::Conflict(
						"prepared executor subject already differs".into(),
					));
				}
			} else {
				access.snapshot.bundle.subjects.insert(subject, value);
				save_bundle(&mut access).await?;
			}
			Ok(())
		}
		.await;
		access.finish(result).await?;
	}
	Ok(Json(prepared))
}
async fn save_bundle(access: &mut Access) -> Result<()> {
	access.snapshot.bundle.validate()?;
	access.snapshot.revision = access
		.snapshot
		.revision
		.checked_add(1)
		.ok_or_else(|| Error::Invalid("authorization revision exhausted".into()))?;
	sqlx::query(
		&Query::update()
			.table(Alias::new("authorization_bundles"))
			.value(Alias::new("revision"), Expr::cust("$2"))
			.value(Alias::new("document"), Expr::cust("$3"))
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::cust("tenant=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(access.snapshot.revision)
	.bind(json!(access.snapshot.bundle))
	.execute(&mut **access.tx)
	.await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("authorization_revisions"))
			.columns(["tenant", "revision", "document", "actor"].map(Alias::new))
			.values_panic(["$1", "$2", "$3", "$4"].map(Expr::cust))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(access.snapshot.revision)
	.bind(json!(access.snapshot.bundle))
	.bind(&access.identity.subject)
	.execute(&mut **access.tx)
	.await?;
	Ok(())
}
pub(crate) async fn describe(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Reference>,
) -> Result<Json<Intent>> {
	let source = crate::api::peer_node(&headers)?;
	let (access, intent) = home_lease(&f, source, input.intent_id, false).await?;
	access.finish(Ok(Json(intent))).await
}
pub(crate) async fn prepare(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Reference>,
) -> Result<Json<Prepared>> {
	let source = crate::api::peer_node(&headers)?;
	Ok(Json(prepare_at(&f, source, input.intent_id).await?))
}
async fn prepare_at(f: &Federation, source: &str, id: Uuid) -> Result<Prepared> {
	let intent: Intent = peer::authority_request(
		f,
		source,
		"/scoped/generation/describe",
		&json!({"intent_id":id}),
	)
	.await?;
	if intent.id != id
		|| intent.home_node != source
		|| intent.target_node != f.config.node_id
		|| intent.expires_at <= Utc::now()
	{
		return Err(Error::Forbidden);
	}
	let mut access = peer::access_mode(
		f,
		source,
		&intent.source_tenant,
		&intent.source_subject,
		true,
	)
	.await?;
	let result = async {
		access
			.require(
				&access.resource("node", &f.config.node_id, json!({})),
				"federation.execute",
			)
			.await?;
		access
			.require(
				&access.resource(
					"task",
					format!("{source}/tasks/{}", intent.task.id),
					json!({}),
				),
				"task.read",
			)
			.await?;
		access
			.require(
				&super::resource(&access, &intent.policy_id),
				"generation.request",
			)
			.await?;
		access
			.require(
				&super::resource(&access, &intent.policy_id),
				"generation.read",
			)
			.await?;
		let policy = policy::load(
			&mut access.tx,
			&access.identity.tenant,
			&intent.policy_id,
			true,
		)
		.await?;
		if policy.revision != intent.policy_revision || !policy.spec.enabled {
			return Err(Error::Conflict("generation policy revision changed".into()));
		}
		let spec = policy.spec.clone();
		let existing: Option<Request> = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("generation_requests"))
				.and_where(Expr::cust("home_node=$1 AND task_id=$2"))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(source)
		.bind(intent.task.id)
		.fetch_optional(&mut **access.tx)
		.await?;
		let mut job = if let Some(job) = existing {
			if job.foreign_intent != Some(json!(intent))
				|| job.credential_id != access.identity.credential_id
				|| job.subject_chain != access.subjects
				|| job.tenant != access.identity.tenant
			{
				return Err(Error::Conflict(
					"foreign generation already has a different binding".into(),
				));
			}
			job
		} else {
			let Assignment::Generated { generation } = super::create_in(
				f,
				&mut access,
				&intent.task,
				policy,
				&intent.reason,
				Some(&intent),
			)
			.await?
			else {
				return Err(Error::Forbidden);
			};
			*generation
		};
		if job.status == "QUEUED" && !job.prepared {
			super::provision::publish(f, &mut access, &job, &spec).await?;
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_requests"))
					.value(Alias::new("prepared"), true)
					.and_where(Expr::cust("id=$1 AND status='QUEUED'"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(job.id)
			.execute(&mut **access.tx)
			.await?;
			job.prepared = true;
		}
		Ok(Prepared {
			intent_id: id,
			node_id: f.config.node_id.clone(),
			request_id: job.id,
			agent: EntityRef {
				id: job.agent_id,
				version: job.agent_version,
			},
			status: job.status,
			prepared: job.prepared,
			expires_at: job.expires_at,
		})
	}
	.await;
	access.finish(result).await
}

/// Called by the receiver's inspection leaf under current mapped authority.
pub(crate) async fn inspect(
	access: &mut Access,
	source: &str,
	task: Option<Uuid>,
	agent: &EntityRef,
) -> Result<Option<Value>> {
	let job: Option<Request> = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_requests"))
			.and_where(Expr::cust("agent_id=$1 AND agent_version=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&agent.id)
	.bind(&agent.version)
	.fetch_optional(&mut **access.tx)
	.await?;
	let Some(job) = job else {
		return Ok(None);
	};
	if job.home_node != source
		|| Some(job.task_id) != task
		|| job.tenant != access.identity.tenant
		|| job.credential_id != access.identity.credential_id
		|| job.subject_chain != access.subjects
		|| !job.prepared
		|| !matches!(job.status.as_str(), "QUEUED" | "ACTIVE")
		|| job.expires_at <= Utc::now()
	{
		return Err(Error::Forbidden);
	}
	Ok(job.foreign_intent)
}
pub(crate) async fn bind(
	f: &Federation,
	access: &mut Access,
	description: &crate::authorization::remote::Description,
	admission: Uuid,
	activate: bool,
) -> Result<()> {
	let Some(value) = &description.inspection.generation else {
		return Ok(());
	};
	let intent: Intent = serde_json::from_value(value.clone())?;
	let job: Request = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_requests"))
			.and_where(Expr::cust("home_node=$1 AND task_id=$2"))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&description.source_node)
	.bind(description.task.id)
	.fetch_one(&mut **access.tx)
	.await?;
	if job.foreign_intent.as_ref() != Some(value)
		|| intent.task.id != description.task.id
		|| job.agent_id != description.inspection.agent.id
		|| job.agent_version != description.inspection.agent.version
		|| job.grant_id.is_some_and(|id| id != description.grant_id)
		|| job.admission_id.is_some_and(|id| id != admission)
		|| !job.prepared
		|| !matches!(job.status.as_str(), "QUEUED" | "ACTIVE")
		|| job.expires_at <= Utc::now()
	{
		return Err(Error::Forbidden);
	}
	sqlx::query(
		&Query::update()
			.table(Alias::new("generation_requests"))
			.value(Alias::new("grant_id"), Expr::cust("$2"))
			.value(Alias::new("admission_id"), Expr::cust("$3"))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(job.id)
	.bind(description.grant_id)
	.bind(admission)
	.execute(&mut **access.tx)
	.await?;
	if activate && job.status == "QUEUED" {
		super::lifecycle::transition(
			f,
			&mut access.tx,
			&job,
			"ACTIVE",
			"generation-service",
			"bound foreign admission activated",
		)
		.await?;
	}
	Ok(())
}

/// Home validates the saved intent locally, without a nested callback to B.
pub(crate) async fn check_home(
	access: &mut Access,
	task: &Task,
	node: &str,
	generation: Option<&Value>,
) -> Result<()> {
	let Some(generation) = generation else {
		return Ok(());
	};
	let intent: Intent = serde_json::from_value(generation.clone())?;
	let record: Record = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_remote_intents"))
			.and_where(Expr::cust("id=$1"))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(intent.id)
	.fetch_optional(&mut **access.tx)
	.await?
	.ok_or(Error::Forbidden)?;
	let original = &access.subjects[..access.subjects.len().saturating_sub(1)];
	if record.cancelled
		|| record.binding != *generation
		|| record.credential_id != access.identity.credential_id
		|| record.tenant != access.identity.tenant
		|| record.subject_chain != original
		|| intent.target_node != node
		|| intent.task.id != task.id
		|| intent.task.workspace_id != task.workspace_id
		|| intent.expires_at <= Utc::now()
	{
		return Err(Error::Forbidden);
	}
	home_authority(access, task, node, &intent.policy_id).await
}

pub(crate) async fn require_active(
	access: &mut Access,
	description: &crate::authorization::remote::Description,
	run: Uuid,
) -> Result<()> {
	if description.inspection.generation.is_none() {
		return Ok(());
	}
	let live:bool=sqlx::query_scalar(&Query::select().expr(Expr::cust("COUNT(*)=1")).from(Alias::new("generation_requests"))
    .and_where(Expr::cust("home_node=$1 AND task_id=$2 AND grant_id=$3 AND admission_id=$4 AND status='ACTIVE' AND NOT quota_released AND expires_at>CLOCK_TIMESTAMP()"))
    .to_string(PostgresQueryBuilder)).bind(&description.source_node).bind(description.task.id).bind(description.grant_id).bind(run).fetch_one(&mut **access.tx).await?;
	if !live {
		return Err(Error::Forbidden);
	}
	Ok(())
}
#[utoipa::path(post,path="/tasks/{id}/remote-generation/{intent}/cancel",operation_id="remote_generation_cancel",params(("id"=Uuid,Path),("intent"=Uuid,Path)),responses((status=200,body=bool)),security(("bearer_auth"=[])))]
pub(crate) async fn cancel(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((task, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<bool>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let record = load(&f, id).await?;
	let intent: Intent = serde_json::from_value(record.binding)?;
	if intent.task.id != task
		|| record.tenant != identity.tenant
		|| record.credential_id != identity.credential_id
	{
		return Err(Error::Forbidden);
	}
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		// Cancellation needs management authority, not disclosure of invalid context.
		access
			.require(
				&access.resource(
					"task",
					task,
					json!({"workspace_id":intent.task.workspace_id}),
				),
				"task.delegate",
			)
			.await?;
		sqlx::query(
			&Query::update()
				.table(Alias::new("generation_remote_intents"))
				.value(Alias::new("cancelled"), true)
				.and_where(Expr::cust("id=$1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.execute(&mut **access.tx)
		.await?;
		Ok(())
	}
	.await;
	access.finish(result).await?;
	if let Err(error) = deliver_cancel(&f, id, &intent.target_node).await {
		tracing::warn!(%error, %id, "remote generation cancellation queued for retry");
	}
	Ok(Json(true))
}

async fn deliver_cancel(f: &Federation, id: Uuid, target: &str) -> Result<()> {
	// Reserve a retry slot before sending, so a failed RPC or process exit leaves
	// a durable pending intent without hot-looping the reconciliation worker.
	let reserved = sqlx::query(
		&Query::update()
			.table(Alias::new("generation_remote_intents"))
			.value(Alias::new("cancel_retry_at"), Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 seconds'"))
			.and_where(Expr::cust("id=$1 AND cancelled AND NOT cancel_delivered AND (cancel_retry_at IS NULL OR cancel_retry_at<=CLOCK_TIMESTAMP())"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.execute(&f.store.pool)
	.await?
	.rows_affected();
	if reserved == 0 {
		return Ok(());
	}
	let acknowledged: bool = peer::authority_request(
		f,
		target,
		"/scoped/generation/cancel",
		&json!({"intent_id":id}),
	)
	.await?;
	if !acknowledged {
		return Err(Error::Conflict("remote generation cancellation was not acknowledged".into()));
	}
	sqlx::query(
		&Query::update()
			.table(Alias::new("generation_remote_intents"))
			.value(Alias::new("cancel_delivered"), true)
			.and_where(Expr::cust("id=$1 AND cancelled"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.execute(&f.store.pool)
	.await?;
	Ok(())
}
pub(crate) async fn cancel_at(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Reference>,
) -> Result<Json<bool>> {
	let source = crate::api::peer_node(&headers)?;
	let job: Option<Request> = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_requests"))
			.and_where(Expr::cust("home_node=$1 AND foreign_intent->>'id'=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(source)
	.bind(input.intent_id.to_string())
	.fetch_optional(&f.store.pool)
	.await?;
	if let Some(job) = job {
		terminate(&f, &job, "STOPPED").await?;
	}
	Ok(Json(true))
}
async fn terminate(f: &Federation, job: &Request, status: &str) -> Result<()> {
	let mut tx = f.store.pool.begin().await?;
	crate::authorization::Authorization::load_with_mode(&mut tx, &job.tenant, true).await?;
	let job = super::lifecycle::load(&mut tx, &job.tenant, job.id).await?;
	if matches!(
		job.status.as_str(),
		"PENDING_APPROVAL" | "QUEUED" | "ACTIVE"
	) {
		super::lifecycle::transition(
			f,
			&mut tx,
			&job,
			status,
			"generation-service",
			"foreign generation lifecycle reconciled",
		)
		.await?;
	}
	tx.commit().await?;
	f.notify.notify_waiters();
	Ok(())
}
pub(crate) async fn reconcile(f: &Federation) -> Result<()> {
	let pending: Vec<(Uuid, Value)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("id"), Alias::new("binding")])
			.from(Alias::new("generation_remote_intents"))
			.and_where(Expr::cust("cancelled AND NOT cancel_delivered AND (cancel_retry_at IS NULL OR cancel_retry_at<=CLOCK_TIMESTAMP())"))
			.order_by(Alias::new("cancel_retry_at"), sea_orm::sea_query::Order::Asc)
			.limit(16)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&f.store.pool)
	.await?;
	for (id, binding) in pending {
		let intent: Intent = serde_json::from_value(binding)?;
		if let Err(error) = deliver_cancel(f, id, &intent.target_node).await {
			tracing::warn!(%error, %id, "remote generation cancellation retry failed");
		}
	}
	let jobs:Vec<Request>=sqlx::query_as(&Query::select().column((Alias::new("g"),Asterisk)).from_as(Alias::new("generation_requests"),Alias::new("g"))
    .join_as(sea_orm::sea_query::JoinType::LeftJoin,Alias::new("runs"),Alias::new("r"),Expr::cust("r.id=g.admission_id AND r.home_node=g.home_node"))
    .and_where(Expr::cust("g.home_node<>'' AND g.status IN ('PENDING_APPROVAL','QUEUED','ACTIVE') AND (g.expires_at<=CLOCK_TIMESTAMP() OR r.phase IN ('COMPLETED','FAILED','CANCELLED'))"))
    .order_by((Alias::new("g"),Alias::new("created_at")),sea_orm::sea_query::Order::Asc).limit(16).to_string(PostgresQueryBuilder)).fetch_all(&f.store.pool).await?;
	for job in jobs {
		let status = if job.expires_at <= Utc::now() {
			"EXPIRED"
		} else if let Some(id) = job.admission_id {
			let run = f.store.run(id).await?;
			match run.phase.as_str() {
				"COMPLETED" => "COMPLETED",
				"CANCELLED" => "STOPPED",
				_ => "FAILED",
			}
		} else {
			"EXPIRED"
		};
		terminate(f, &job, status).await?;
	}
	Ok(())
}

pub(crate) fn check_preparation(task: &Task, generation: Option<&Value>) -> Result<()> {
	if let Some(value) = generation {
		let intent: Intent = serde_json::from_value(value.clone())?;
		if intent.task.id != task.id || intent.task.revision != task.revision {
			return Err(Error::Conflict(
				"generation intent task revision changed before grant preparation".into(),
			));
		}
	}
	Ok(())
}
