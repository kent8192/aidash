//! Subject authority is fixed by a durable participant reservation. Source
//! attempts survive process loss so a lost reply cannot complete revocation.
use super::{Manifest, Mutation, Status, coordinator, gate};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	federation::Federation,
};
use sea_orm::sea_query::{Alias, Asterisk, Expr, OnConflict, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Origin {
	credential_id: Uuid,
	tenant: String,
	subject: String,
}
impl From<&SubjectIdentity> for Origin {
	fn from(identity: &SubjectIdentity) -> Self {
		Self {
			credential_id: identity.credential_id,
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		}
	}
}
impl Origin {
	fn identity(&self) -> SubjectIdentity {
		SubjectIdentity {
			credential_id: self.credential_id,
			tenant: self.tenant.clone(),
			subject: self.subject.clone(),
		}
	}
}

/// No state, artifact content, registry document or provider argument is sent
/// during preflight. Resource identifiers and the recipient set are explicit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Target {
	kind: String,
	id: Uuid,
	task_id: Option<Uuid>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preflight {
	id: Uuid,
	coordinator: String,
	digest: String,
	origin: Origin,
	recipients: Vec<String>,
	targets: Vec<Target>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Binding {
	request: Preflight,
	local: Origin,
	subjects: Vec<String>,
}

pub(crate) async fn control(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
	sqlx::query(
		&Query::select()
			.expr(Expr::cust(
				"set_config('aidash.transaction_control','authority',true)",
			))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
fn control_federation(f: &Federation) -> Federation {
	let mut f = f.clone();
	f.store.pool = f.store.control_pool.clone();
	f
}
async fn access(f: &Federation, origin: &Origin) -> Result<Access> {
	let mut access = Access::begin(&control_federation(f).store, &origin.identity()).await?;
	control(&mut access.tx).await?;
	Ok(access)
}
pub(super) async fn bind(
	tx: &mut Transaction<'_, Postgres>,
	table: &str,
	id: Uuid,
	binding: &Value,
) -> Result<()> {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new(table))
			.columns([Alias::new("id"), Alias::new("binding")])
			.values_panic([Expr::cust("$1"), Expr::cust("$2")])
			.on_conflict(OnConflict::new().do_nothing().to_owned())
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(binding)
	.execute(&mut **tx)
	.await?;
	let stored: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("binding"))
			.from(Alias::new(table))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_one(&mut **tx)
	.await?;
	if stored != *binding {
		return Err(Error::Conflict(
			"transaction authority binding is immutable".into(),
		));
	}
	Ok(())
}
async fn binding<T: serde::de::DeserializeOwned>(
	f: &Federation,
	table: &str,
	id: Uuid,
) -> Result<Option<T>> {
	let value: Option<Value> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("binding"))
			.from(Alias::new(table))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_optional(&f.store.control_pool)
	.await?;
	value
		.map(serde_json::from_value)
		.transpose()
		.map_err(Into::into)
}
pub(super) async fn match_origin(f: &Federation, id: Uuid, origin: Option<&Origin>) -> Result<()> {
	let stored: Option<Origin> = binding(f, "atomic_subjects", id).await?;
	if stored.as_ref() != origin {
		return Err(Error::Forbidden);
	}
	Ok(())
}
fn request(manifest: &Manifest, origin: &Origin, node: &str) -> Result<Preflight> {
	let targets = manifest
		.local(node)?
		.mutations
		.iter()
		.map(|m| {
			Ok(match m {
				Mutation::WorkspaceState { workspace_id, .. } => Target {
					kind: "workspace".into(),
					id: *workspace_id,
					task_id: None,
				},
				Mutation::CompleteTask { task_id, .. } => Target {
					kind: "task".into(),
					id: *task_id,
					task_id: None,
				},
				Mutation::FinishRun {
					run_id, task_id, ..
				} => Target {
					kind: "run".into(),
					id: *run_id,
					task_id: Some(*task_id),
				},
				Mutation::RegistryRegister { .. } => return Err(Error::Forbidden),
			})
		})
		.collect::<Result<Vec<_>>>()?;
	Ok(Preflight {
		id: manifest.id,
		coordinator: manifest.coordinator.clone(),
		digest: manifest.digest()?,
		origin: origin.clone(),
		recipients: manifest
			.participants
			.iter()
			.map(|p| p.node_id.clone())
			.collect(),
		targets,
	})
}

async fn checks(access: &mut Access, input: &Preflight, action: &str) -> Result<()> {
	let resource = access.resource(
		"transaction",
		input.id,
		json!({"coordinator":input.coordinator,"participants":input.recipients}),
	);
	access.require(&resource, action).await?;
	// Establish the complete stored chain before evaluating any mutation.
	for target in &input.targets {
		match target.kind.as_str() {
			"task" => {
				crate::authorization::execution::inherit_task_origin(access, target.id).await?;
			}
			"run" => {
				let run = run(access, target.id).await?;
				if Some(run.task_id) != target.task_id {
					return Err(Error::Forbidden);
				}
				inherit_run(access, &run, input).await?;
			}
			"workspace" => {}
			_ => return Err(Error::Invalid("unsupported transaction target".into())),
		}
	}
	access.require(&resource, action).await?;
	for target in &input.targets {
		let resource = match target.kind.as_str() {
			"workspace" => {
				let resource = access.workspace(target.id).await?;
				access.require(&resource, "workspace.read").await?;
				if action != "transaction.read" {
					access.require(&resource, "workspace.update").await?;
				}
				resource
			}
			"task" => {
				let task = access.task_read(target.id).await?;
				let resource = access.task_resource(&task).await?;
				if action != "transaction.read" {
					access.require(&resource, "task.complete").await?;
					let artifact = access
						.artifact_creation_resource(task.id, task.owner.as_deref().unwrap_or(""))
						.await?;
					access.require(&artifact, "artifact.create").await?;
				}
				resource
			}
			"run" => {
				let run = run(access, target.id).await?;
				let resource = access.resource(
					"run",
					run.id,
					json!({"task_id":run.task_id,"workspace_id":run.workspace_id,"home_node":run.home_node}),
				);
				access.require(&resource, "run.read").await?;
				if action != "transaction.read" {
					access.require(&resource, "run.finish").await?;
				}
				resource
			}
			_ => return Err(Error::Forbidden),
		};
		if action != "transaction.read" {
			for recipient in &input.recipients {
				let mut disclosure = resource.clone();
				disclosure.attributes["recipient_node"] = json!(recipient);
				access.require(&disclosure, "transaction.disclose").await?;
			}
		}
	}
	Ok(())
}
async fn inherit_run(
	access: &mut Access,
	run: &crate::domain::Run,
	input: &Preflight,
) -> Result<()> {
	let previous = access.subjects.clone();
	if access.context["source_node"].as_str() != Some(run.home_node.as_str()) {
		crate::authorization::execution::inherit_run_authority(access, run).await?;
	} else {
		let record: Option<(String, Uuid, Vec<String>, Value)> = sqlx::query_as(
			&Query::select()
				.columns([
					Alias::new("tenant"),
					Alias::new("credential_id"),
					Alias::new("subject_chain"),
					Alias::new("description"),
				])
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(Expr::cust("id=$1 AND source_node=$2 AND task_id=$3"))
				.lock(sea_orm::sea_query::LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(&input.coordinator)
		.bind(run.task_id)
		.fetch_optional(&mut **access.tx)
		.await?;
		let (tenant, credential, chain, description) = record.ok_or(Error::Forbidden)?;
		let description: crate::authorization::remote::Description =
			serde_json::from_value(description)?;
		if tenant != access.identity.tenant
			|| credential != access.identity.credential_id
			|| description.source_tenant != input.origin.tenant
			|| description.source_subject != input.origin.subject
			|| description.task.workspace_id != run.workspace_id
			|| description.inspection.agent.id != run.agent_id
			|| description.inspection.agent.version != run.agent_version
			|| chain.first() != Some(&access.identity.subject)
		{
			return Err(Error::Forbidden);
		}
		access.subjects = chain;
	}
	if previous.len() > 1 && previous != access.subjects {
		return Err(Error::Forbidden);
	}
	Ok(())
}

async fn run(access: &mut Access, id: Uuid) -> Result<crate::domain::Run> {
	sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_optional(&mut **access.tx)
	.await?
	.ok_or(Error::Forbidden)
}
async fn mapped(f: &Federation, input: &Preflight) -> Result<Access> {
	if input.coordinator == f.config.node_id {
		return access(f, &input.origin).await;
	}
	let mut access = crate::authorization::peer::access(
		&control_federation(f),
		&input.coordinator,
		&input.origin.tenant,
		&input.origin.subject,
	)
	.await?;
	control(&mut access.tx).await?;
	Ok(access)
}
pub(super) async fn preflight(f: &Federation, caller: &str, input: &Preflight) -> Result<()> {
	if caller != input.coordinator
		|| input.targets.len() > 64
		|| input.recipients.is_empty()
		|| input.recipients.len() > 16
		|| !input.recipients.contains(&f.config.node_id)
		|| input.digest.len() != 64
	{
		return Err(Error::Forbidden);
	}
	let _visibility = gate::ReadLease::begin(&f.store).await?;
	let mut access = mapped(f, input).await?;
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
	checks(
		access,
		&request(manifest, origin, &manifest.coordinator)?,
		action,
	)
	.await?;
	// Qualified remote resources require source permission as well as each
	// recipient's independent mapped policy. No caller-supplied owner attrs.
	for participant in &manifest.participants {
		if participant.node_id == manifest.coordinator {
			continue;
		}
		for target in request(manifest, origin, &participant.node_id)?.targets {
			let resource = access.resource(
				&target.kind,
				format!("{}:{}", participant.node_id, target.id),
				json!({"node_id":participant.node_id}),
			);
			access.require(&resource, action).await?;
			for recipient in &manifest.participants {
				if action != "transaction.read" {
					let mut disclosure = resource.clone();
					disclosure.attributes["recipient_node"] = json!(recipient.node_id);
					access.require(&disclosure, "transaction.disclose").await?;
				}
			}
		}
	}
	Ok(())
}
pub(super) async fn submit(
	f: &Federation,
	identity: &SubjectIdentity,
	manifest: &Manifest,
) -> Result<Status> {
	manifest.validate()?;
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
	match coordinator::status(f, manifest.id).await {
		Ok(existing) => {
			let result = async {
				match_origin(f, manifest.id, Some(&origin)).await?;
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
	let lease = gate::ReadLease::begin(&f.store).await?;
	let result = async {
		for node in &manifest.participants {
			let input = request(manifest, &origin, &node.node_id)?;
			if node.node_id == f.config.node_id {
				let binding = Binding {
					request: input,
					local: origin.clone(),
					subjects: access.subjects.clone(),
				};
				bind(
					&mut access.tx,
					"atomic_preflights",
					manifest.id,
					&json!(binding),
				)
				.await?;
			} else {
				let _: Value = coordinator::remote(
					f,
					&node.node_id,
					reqwest::Method::POST,
					"/transactions/preflight",
					Some(&input),
				)
				.await?;
			}
		}
		Ok(())
	}
	.await;
	// Preflights must be durable before recovery can observe the coordinator.
	// This is not admission; every new reservation checks live authority again.
	access.finish(result).await?;
	let stored = coordinator::submit_bound(f, manifest, Some(&origin)).await?;
	drop(lease);
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

		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("atomic_authority_attempts"))
				.columns([Alias::new("transaction_id"), Alias::new("node_id")])
				.values_panic([Expr::cust("$1"), Expr::cust("$2")])
				.on_conflict(OnConflict::new().do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.bind(manifest.id)
		.bind(node)
		.execute(&mut **access.tx)
		.await?;
		Ok(())
	}
	.await;
	super::fault::cut(manifest.id, "authority.issue.before").await?;
	access.finish(result).await?;
	super::fault::cut(manifest.id, "authority.issue.after").await
}
pub(super) async fn ticket(f: &Federation, id: Uuid, node: &str) -> Result<Preflight> {
	let origin = binding::<Origin>(f, "atomic_subjects", id)
		.await?
		.ok_or(Error::Forbidden)?;
	let mut access = access(f, &origin).await?;
	let result = async {
		trusted(&mut access, node).await?;
		let state = coordinator::status(f, id).await?;
		if state.decision.is_some() {
			return Err(Error::Forbidden);
		}
		let issued: Option<String> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("node_id"))
				.from(Alias::new("atomic_authority_attempts"))
				.and_where(Expr::cust(
					"transaction_id=$1 AND node_id=$2 AND outcome IS NULL",
				))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(node)
		.fetch_optional(&mut **access.tx)
		.await?;
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
pub(super) async fn settle(f: &Federation, id: Uuid, node: &str, outcome: &str) -> Result<()> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("atomic_authority_attempts"))
			.value(Alias::new("outcome"), Expr::cust("$3"))
			.and_where(Expr::cust(
				"transaction_id=$1 AND node_id=$2 AND outcome IS NULL",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(node)
	.bind(outcome)
	.execute(&f.store.control_pool)
	.await?;
	Ok(())
}
pub(super) async fn manage(
	f: &Federation,
	identity: &SubjectIdentity,
	state: &Status,
	action: &str,
) -> Result<()> {
	let origin = binding::<Origin>(f, "atomic_subjects", state.id)
		.await?
		.ok_or(Error::Forbidden)?;
	if origin.tenant != identity.tenant || origin.subject != identity.subject {
		return Err(Error::Forbidden);
	}
	let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
	let mut access = access(f, &Origin::from(identity)).await?;
	let result = async {
		source_checks(&mut access, &manifest, &origin, "transaction.read").await?;
		if action != "transaction.read" {
			let resource = access.resource("transaction", state.id, json!({}));
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
		if action == "transaction.abort" {
			coordinator::abort(f, state.id).await?;
		}
		Ok(())
	}
	.await;
	access.finish(result).await
}
pub(super) async fn read_access(f: &Federation, caller: &str, input: &Preflight) -> Result<()> {
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
	if Origin::from(&access.identity) != bound.local {
		return Err(Error::Forbidden);
	}
	let result = checks(&mut access, input, "transaction.read").await;
	access.finish(result).await
}

/// Conservative unknown outcomes are safe to display as pending even before
/// revocation. A terminal acknowledgment is the only completion evidence.
pub(crate) async fn pending(
	f: &Federation,
	tenant: &str,
	credential: Option<Uuid>,
) -> Result<Vec<Uuid>> {
	let mut query = Query::select();
	query
		.distinct()
		.column((Alias::new("a"), Alias::new("transaction_id")))
		.from_as(Alias::new("atomic_authority_attempts"), Alias::new("a"))
		.join_as(
			sea_orm::sea_query::JoinType::InnerJoin,
			Alias::new("atomic_subjects"),
			Alias::new("s"),
			Expr::cust("s.id=a.transaction_id"),
		)
		.and_where(Expr::cust("a.outcome IS NULL AND s.binding->>'tenant'=$1"));
	if credential.is_some() {
		query.and_where(Expr::cust("s.binding->>'credential_id'=$2"));
	}
	let sql = query.to_string(PostgresQueryBuilder);
	let mut query = sqlx::query_scalar(&sql).bind(tenant);
	if let Some(credential) = credential {
		query = query.bind(credential.to_string());
	}
	Ok(query.fetch_all(&f.store.control_pool).await?)
}

pub(super) async fn scoped(f: &Federation, id: Uuid) -> Result<bool> {
	Ok(binding::<Origin>(f, "atomic_subjects", id).await?.is_some())
}

async fn trusted(access: &mut Access, node: &str) -> Result<()> {
	let trusted: Option<bool> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("enabled"))
			.from(Alias::new("atomic_peer_trust"))
			.and_where(Expr::cust("node_id=$1"))
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(node)
	.fetch_optional(&mut **access.tx)
	.await?;
	if trusted != Some(true) {
		return Err(Error::Forbidden);
	}
	Ok(())
}
pub(super) async fn pending_peer(f: &Federation, node: &str) -> Result<Vec<Uuid>> {
	Ok(sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("transaction_id"))
			.from(Alias::new("atomic_authority_attempts"))
			.and_where(Expr::cust("node_id=$1 AND outcome IS NULL"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(node)
	.fetch_all(&f.store.control_pool)
	.await?)
}
