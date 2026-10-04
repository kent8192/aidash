//! Receiver admission binds a source grant to one local identity and executor.
//! The record is not an executable run: worker activation must consume this
//! authority through scoped home commands, never through legacy offers.
use super::execution::{InspectInput, inspect_in};
use crate::apps::federation::peer::models::AuthorizationRemoteAdmission;
use crate::{
	Error, Result,
	authorization::{access::Access, remote::Description},
	federation::Federation,
	registry::EntityRef,
};
use reinhardt::db::orm::Json as OrmJson;
use reinhardt::injectable;
use reinhardt::query::Expr;
use reinhardt::query::PostgresQueryBuilder;
use reinhardt::query::Query;

use reinhardt::query::QueryStatementBuilder as _;
use serde_json::json;
use uuid::Uuid;

fn matches(record: &AuthorizationRemoteAdmission, expected: &AuthorizationRemoteAdmission) -> bool {
	record.source_node == expected.source_node
		&& record.grant_id == expected.grant_id
		&& record.task_id == expected.task_id
		&& record.tenant == expected.tenant
		&& record.credential_id == expected.credential_id
		&& record.subject_chain == expected.subject_chain
		&& record.description == expected.description
}

fn proposal(access: &Access, description: &Description) -> Result<AuthorizationRemoteAdmission> {
	Ok(AuthorizationRemoteAdmission::build()
		.id(Uuid::new_v4())
		.source_node(description.source_node.clone())
		.grant_id(description.grant_id)
		.task_id(description.task.id)
		.tenant(access.identity.tenant.clone())
		.credential_id(access.identity.credential_id)
		.subject_chain(access.subjects.clone())
		.description(OrmJson(serde_json::to_value(description)?))
		.finish())
}

async fn lease(f: &Federation, source: &str, grant: Uuid) -> Result<(Access, Description)> {
	// Do not hold receiver authority while calling home: home's verification
	// inspects this receiver too, and a queued policy writer must not deadlock
	// two nested shared leases. Recheck the receiver under a fresh lease after
	// the source reply and retain it until the local binding is committed.
	let description: Description = super::authority_request(
		f,
		source,
		"/scoped/execution/grants/describe",
		&json!({"grant_id":grant}),
	)
	.await?;
	if description.source_node != source
		|| description.target_node != f.config.node_id
		|| description.grant_id != grant
		|| description.task.status != crate::domain::TaskStatus::Open
	{
		return Err(Error::Forbidden);
	}
	receiver_lease(f, source, description).await
}

pub(crate) use crate::apps::identity::serializers::peer_admission::{Admission, Input};

#[derive(Clone)]
pub struct PeerAdmissions {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_admissions(#[inject] runtime: Federation) -> PeerAdmissions {
	PeerAdmissions { runtime }
}

impl PeerAdmissions {
	pub(crate) async fn admit(&self, headers: HeaderMap, input: Input) -> Result<Admission> {
		let f = self.runtime.clone();
		let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
		let (mut access, description) = lease(&f, source, input.grant_id).await?;
		let result = async {
        let agent: AgentConfig = serde_json::from_value(description.inspection.agent.config.clone())?;
        require_workspace_agent(&agent)?;
		{ let query_bind_1 = format!("{source}:{}", description.task.id); sqlx::query(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED(?, 71003209)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.execute(&mut **access.tx)
		.await? };
		let legacy: bool = { let query_bind_1 = source; let query_bind_2 = description.task.id; let query_bind_3 = input.grant_id; sqlx::query_scalar(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM runs r WHERE home_node = ? AND task_id = ? AND NOT EXISTS(SELECT 1 FROM authorization_remote_admissions a WHERE a.id = r.id AND a.source_node = r.home_node AND a.grant_id = ?)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_one(&mut **access.tx)
		.await? };
		if legacy {
			return Err(Error::Conflict(
				"task already has an incompatible execution".into(),
			));
		}
		let existing: Option<Uuid> = { let query_bind_1 = source; let query_bind_2 = input.grant_id; sqlx::query_scalar(&Query::select().column(Alias::new("id"))
			.from(Alias::new("authorization_remote_admissions")).and_where(SimpleExpr::CustomWithExpr("(source_node=? AND grant_id=?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
			.to_string(PostgresQueryBuilder)).fetch_optional(&mut **access.tx).await? };
		if existing.is_none() && !crate::marketplace::active(&mut access,&description.inspection.agent).await? {
			return Err(Error::Forbidden);
		}
		let proposed = Uuid::new_v4();
		{ let query_bind_1 = proposed; let query_bind_2 = source; let query_bind_3 = input.grant_id; let query_bind_4 = description.task.id; let query_bind_5 = &access.identity.tenant; let query_bind_6 = access.identity.credential_id; let query_bind_7 = &access.subjects; let query_bind_8 = serde_json::to_value(&description)?; sqlx::query(&format!("{} ON CONFLICT DO NOTHING", reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new(
					"authorization_remote_admissions",
				))
				.columns([
					reinhardt::query::Alias::new("id"),
					reinhardt::query::Alias::new("source_node"),
					reinhardt::query::Alias::new("grant_id"),
					reinhardt::query::Alias::new("task_id"),
					reinhardt::query::Alias::new("tenant"),
					reinhardt::query::Alias::new("credential_id"),
					reinhardt::query::Alias::new("subject_chain"),
					reinhardt::query::Alias::new("description"),
				])
				.from_subquery(reinhardt::query::Query::select().expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_2.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_6.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![crate::database::text_array(query_bind_7.to_owned())])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_8.to_owned()).into()])).to_owned()).to_owned().to_string(reinhardt::query::PostgresQueryBuilder)))
		.execute(&mut **access.tx)
		.await? };
		let record: Record = { let query_bind_1 = source; let query_bind_2 = input.grant_id; sqlx::query_as(&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new(
					"authorization_remote_admissions",
				))
				.and_where(SimpleExpr::CustomWithExpr("(source_node = ? AND grant_id = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.lock(reinhardt::query::LockType::Share)
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_optional(&mut **access.tx)
		.await? }
		.ok_or_else(|| Error::Conflict("source task already has a different admission".into()))?;
		if !record.matches(&access, &description)? {
			return Err(Error::Conflict(
				"admission already binds different authority".into(),
			));
		}
		let live: bool = { let query_bind_1 = description.expires_at; sqlx::query_scalar(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(CAST(? AS TIMESTAMPTZ) > CLOCK_TIMESTAMP())".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_one(&mut **access.tx)
		.await? };
		if !live {
			return Err(Error::Forbidden);
		}
		crate::generation::foreign::bind(&f, &mut access, &description, record.id, false).await?;
		// Policy decisions are retained by Access. No unscoped workspace event
		// may disclose this source task to receiver tenants.
		Ok(record.view(&description))
	}
	.await;
		access.finish(result).await
	}

	pub(crate) async fn verify(&self, headers: HeaderMap, id: Uuid) -> Result<bool> {
		let f = self.runtime.clone();
		let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
		let connection = f.store.orm_connection()?;
		let record = AuthorizationRemoteAdmission::by_id(&mut connection.handle(), id, source)
			.await?
			.ok_or(Error::Forbidden)?;
		let (access, description) = lease(&f, source, record.grant_id).await?;
		let result = if matches(&record, &proposal(&access, &description)?) {
			Ok(true)
		} else {
			Err(Error::Forbidden)
		};
		access.finish(result).await
	}
}

use reinhardt::query::ExprTrait as _;

pub(crate) async fn message(
	f: Federation,
	headers: HeaderMap,
	id: Uuid,
	input: MessageInput,
) -> Result<crate::authorization::remote::execution::RemoteExecutionMessageReceipt> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	let run = f.store.run(id).await?;
	if run.home_node != source
		|| run_grant(&f.store, &run.metadata()).await? != Some(input.grant_id)
	{
		return Err(Error::Forbidden);
	}
	let (mut access, _) = worker_lease(&f, &run.metadata())
		.await?
		.ok_or(Error::Forbidden)?;
	let preflight = async {
		access
			.require(&access.resource("run", id, json!({})), "run.message")
			.await?;
		access
			.require(
				&access.resource(
					"workspace",
					format!("{source}/workspaces/{}", run.workspace_id),
					json!({}),
				),
				"message.create",
			)
			.await?;
		Ok(())
	}
	.await;
	access.finish(preflight).await?;
	let home = crate::federation::Home::new(f.clone(), run.clone());
	let key = format!("human:{id}:{}", input.message.id);
	let limit = f.run_message_limit(&run).await?;
	if !home
		.reserve_run_message(&key, &input.message.content)
		.await?
	{
		return Err(Error::Forbidden);
	}
	// Reacquire receiver authority after reserving at home. A revocation
	// between reservation and admission cannot create an executable input.
	let (access, _) = worker_lease(&f, &run.metadata())
		.await?
		.ok_or(Error::Forbidden)?;
	let sender = access.identity.subject.clone();
	let mut access = access.into_native()?;
	let result = f
		.store
		.accept_run_message_in(
			access.tx.as_mut(),
			id,
			&sender,
			&input.message.content,
			&key,
			limit,
		)
		.await;
	if let Err(error) = access.finish(result).await
		&& f.store
			.run_input_sequence(id, &key, &input.message.content)
			.await
			.is_err()
	{
		let _ = home.release_run_messages(std::slice::from_ref(&key)).await;
		return Err(error);
	}
	home.commit_run_message(&key, &input.message.content)
		.await?;
	f.deliver_run_messages(&run).await?;
	f.notify.notify_waiters();
	Ok(
		crate::authorization::remote::execution::RemoteExecutionMessageReceipt {
			id: input.message.id,
			run_id: id,
			accepted: true,
		},
	)
}

pub(crate) async fn control(
	f: Federation,
	headers: HeaderMap,
	id: Uuid,
	input: RemoteExecutionControlInput,
) -> Result<crate::authorization::remote::execution::RemoteExecutionActivation> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	let run = f.store.inspect_run(id).await?;
	if run.home_node != source || run_grant(&f.store, &run.metadata).await? != Some(input.grant_id)
	{
		return Err(Error::Forbidden);
	}
	if matches!(
		input.action,
		crate::authorization::remote::execution::RemoteExecutionControl::Resume
	) {
		let (access, _) = worker_lease(&f, &run.metadata)
			.await?
			.ok_or(Error::Forbidden)?;
		access.finish(Ok(())).await?;
	}
	let run = if matches!(
		input.action,
		crate::authorization::remote::execution::RemoteExecutionControl::Cancel
	) && run.control == crate::domain::RunControl::Cancelled
	{
		run
	} else {
		f.store.control(id, input.action.action()).await?
	};
	f.notify.notify_waiters();
	Ok(
		crate::authorization::remote::execution::RemoteExecutionActivation {
			grant_id: input.grant_id,
			admission_id: id,
			run_id: id,
			phase: run.phase().into(),
			control: run.control.into(),
			semantic_reason: run.recovery.as_ref().and_then(|r| r.semantic_reason),
			error: run
				.error
				.clone()
				.map(|_| "Remote execution requires attention.".into()),
		},
	)
}

pub(crate) async fn status(
	f: Federation,
	headers: HeaderMap,
	input: Input,
) -> Result<Option<crate::authorization::remote::execution::RemoteExecutionActivation>> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	let record: Option<Record> = {
		let query_bind_1 = source;
		let query_bind_2 = input.grant_id;
		sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(source_node=? AND grant_id=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&f.store.pool)
		.await?
	};
	let Some(record) = record else {
		return Ok(None);
	};
	let raw: Option<crate::domain::run_state::RawRun> = {
		let query_bind_1 = record.id;
		let query_bind_2 = source;
		aidash_server::database::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=? AND home_node=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&f.store.pool)
		.await?
	};
	let run = raw.map(crate::domain::run_state::RawRun::inspect);
	Ok(Some(
		crate::authorization::remote::execution::RemoteExecutionActivation {
			grant_id: input.grant_id,
			admission_id: record.id,
			run_id: record.id,
			phase: run.as_ref().map_or(
				crate::authorization::remote::execution::RemoteExecutionPhase::Admitted,
				|r| r.phase().into(),
			),
			control: run.as_ref().map_or(
				crate::authorization::remote::execution::RemoteExecutionControlState::Inactive,
				|r| r.control.into(),
			),
			semantic_reason: run
				.as_ref()
				.and_then(|r| r.recovery.as_ref())
				.and_then(|r| r.semantic_reason),
			error: run.and_then(|r| {
				r.error.clone().map(|_| {
					"Remote execution requires attention. Review current authority and retry controls.".into()
				})
			}),
		},
	))
}

fn require_workspace_agent(agent: &AgentConfig) -> Result<()> {
	// Local working areas require a local conversation; a foreign workspace
	// grant cannot manufacture that conversation or inherit its private files.
	if agent.core_capabilities.enabled() {
		return Err(Error::Invalid("remote execution requires a workspace Agent without local core working-area capabilities; transfer files into an explicitly admitted local thread".into()));
	}
	Ok(())
}

/// A scoped record is never treated as a missing legacy grant, even after expiry.
pub(crate) async fn run_grant(
	store: &crate::store::Store,
	run: &crate::domain::RunMetadata,
) -> Result<Option<Uuid>> {
	let record: Option<Record> = {
		let query_bind_1 = run.id;
		sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&store.pool)
		.await?
	};
	let Some(record) = record else {
		return Ok(None);
	};
	let d: Description = serde_json::from_value(record.description)?;
	if record.source_node != run.home_node
		|| record.task_id != run.task_id
		|| d.task.workspace_id != run.workspace_id
		|| d.inspection.agent.id != run.agent_id
		|| d.inspection.agent.version != run.agent_version
	{
		return Err(Error::Forbidden);
	}
	Ok(Some(record.grant_id))
}

pub(crate) async fn worker_lease(
	f: &Federation,
	run: &crate::domain::RunMetadata,
) -> Result<Option<(Access, AgentConfig)>> {
	let Some(grant) = run_grant(&f.store, run).await? else {
		return Ok(None);
	};
	let result = lease(f, &run.home_node, grant).await;
	let (mut access, d) = match result {
		Ok(value) => value,
		Err(error) => {
			let document: Value = {
				let query_bind_1 = run.id;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("description"))
						.from(Alias::new("authorization_remote_admissions"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&f.store.pool)
				.await?
			};
			let description: Description = serde_json::from_value(document)?;
			return Err(if description.semantic.disabled() {
				error
			} else {
				Error::RemoteSemantic(crate::authorization::remote::semantic::failure(&error))
			});
		}
	};
	let result = async {
		let record: Record = {
			let query_bind_1 = run.id;
			sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **access.tx)
			.await?
		};
		if !record.matches(&access, &d)? {
			return Err(Error::Forbidden);
		}
		crate::generation::foreign::require_active(&mut access, &d, run.id).await?;
		let agent: AgentConfig = serde_json::from_value(d.inspection.agent.config)?;
		access.durable_audit = true;
		access.worker();
		Ok(agent)
	}
	.await;
	match result {
		Ok(agent) => Ok(Some((access, agent))),
		Err(e) => access.finish(Err(e)).await,
	}
}

pub(crate) async fn activate(
	f: Federation,
	headers: HeaderMap,
	id: Uuid,
	input: Input,
) -> Result<crate::authorization::remote::execution::RemoteExecutionActivation> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	let bound: Uuid = super::authority_request(
		&f,
		source,
		"/scoped/execution/grants/activation",
		&json!({"grant_id":input.grant_id}),
	)
	.await?;
	if bound != id {
		return Err(Error::Forbidden);
	}
	let (mut access, d) = lease(&f, source, input.grant_id).await?;
	let result = async {
		let record: Record = {
			let query_bind_1 = id;
			sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
		}
		.ok_or(Error::Forbidden)?;
		if !record.matches(&access, &d)? {
			return Err(Error::Forbidden);
		}
		crate::generation::foreign::bind(&f, &mut access, &d, id, true).await?;
		let agent: AgentConfig = serde_json::from_value(d.inspection.agent.config.clone())?;
		require_workspace_agent(&agent)?;
		{
			let query_bind_1 = format!("{source}:{}", d.task.id);
			sqlx::query(
				&Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED(?, 71003209)))".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **access.tx)
			.await?
		};
		sqlx::query(&format!(
			"{} ON CONFLICT DO NOTHING",
			Query::insert()
				.into_table(Alias::new("runs"))
				.columns(
					[
						"id",
						"task_id",
						"workspace_id",
						"home_node",
						"agent_id",
						"agent_version",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::cust("$1"))
						.expr(Expr::cust("$2"))
						.expr(Expr::cust("$3"))
						.expr(Expr::cust("$4"))
						.expr(Expr::cust("$5"))
						.expr(Expr::cust("$6"))
						.to_owned()
				)
				.to_owned()
				.to_string(PostgresQueryBuilder)
		))
		.bind(id)
		.bind(d.task.id)
		.bind(d.task.workspace_id)
		.bind(source)
		.bind(&d.inspection.agent.id)
		.bind(&d.inspection.agent.version)
		.execute(&mut **access.tx)
		.await?;
		let run: Run = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("runs"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
		}
		.ok_or_else(|| Error::Conflict("source task already has another execution".into()))?;
		if run.task_id != d.task.id
			|| run.workspace_id != d.task.workspace_id
			|| run.home_node != source
			|| run.agent_id != d.inspection.agent.id
			|| run.agent_version != d.inspection.agent.version
		{
			return Err(Error::Forbidden);
		}
		Ok(
			crate::authorization::remote::execution::RemoteExecutionActivation {
				grant_id: input.grant_id,
				admission_id: id,
				run_id: id,
				phase: run.phase().into(),
				control: run.control.into(),
				semantic_reason: run.recovery.semantic_reason,
				error: run.error,
			},
		)
	}
	.await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(result)
}

use crate::domain::Run;
use crate::registry::AgentConfig;

use http::HeaderMap;

#[derive(Clone)]
pub struct PeerExecutionManagement {
	pub(crate) runtime: Federation,
}
// Preserve a statement before the value until reinhardt-web#6441 is fixed.
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PeerExecutionManagement {
	tracing::trace!(
		service = "PeerExecutionManagement",
		"creating injectable service"
	);
	PeerExecutionManagement { runtime }
}

#[derive(sqlx::FromRow)]
struct Record {
	id: Uuid,
	source_node: String,
	grant_id: Uuid,
	task_id: Uuid,
	tenant: String,
	credential_id: Uuid,
	subject_chain: Vec<String>,
	description: Value,
}

use reinhardt::query::Alias;

use reinhardt::query::LockType;

use serde_json::Value;

impl Record {
	fn matches(&self, access: &Access, description: &Description) -> Result<bool> {
		Ok(self.source_node == description.source_node
			&& self.grant_id == description.grant_id
			&& self.task_id == description.task.id
			&& self.tenant == access.identity.tenant
			&& self.credential_id == access.identity.credential_id
			&& self.subject_chain == access.subjects
			&& self.description == serde_json::to_value(description)?)
	}
}

pub(crate) use crate::apps::identity::serializers::peer_admission::{
	MessageInput, RemoteExecutionControlInput,
};

async fn receiver_lease(
	f: &Federation,
	source: &str,
	description: Description,
) -> Result<(Access, Description)> {
	let mut access = super::access(
		f,
		source,
		&description.source_tenant,
		&description.source_subject,
	)
	.await?;
	let result = async {
		access.context["source_workspace_id"] = json!(description.task.workspace_id);
		access.context["source_task_id"] = json!(description.task.id);
		access.context["source_task_revision"] = json!(description.task.revision);
		let fresh = inspect_in(
			f,
			&mut access,
			source,
			&InspectInput {
				task_id: Some(description.task.id),
				tenant: description.source_tenant.clone(),
				subject: description.source_subject.clone(),
				agent: EntityRef {
					id: description.inspection.agent.id.clone(),
					version: description.inspection.agent.version.clone(),
				},
				requirements: serde_json::from_value(description.task.requirements.clone())?,
				compactor: description.semantic.request().compactor().cloned(),
			},
		)
		.await?;
		if !fresh.satisfies(&description.inspection) {
			return Err(Error::Forbidden);
		}
		let workspace = access.resource(
			"workspace",
			format!("{source}/workspaces/{}", description.task.workspace_id),
			json!({}),
		);
		access.require(&workspace, "workspace.read").await?;
		if !description.semantic.disabled() {
			if fresh.semantic_memory != crate::semantic::remote::VERSION {
				return Err(Error::RemoteSemantic(
					crate::semantic::remote::Failure::Configuration,
				));
			}
			access.require(&workspace, "semantic.use").await?;
		}
		let task = access.resource(
			"task",
			format!("{source}/tasks/{}", description.task.id),
			json!({"created_by":description.task.created_by,"requirements":description.task.requirements}),
		);
		access.require(&task, "task.read").await?;
		access.require(&task, "task.execute").await?;
		let live: bool = {
			let query_bind_1 = description.expires_at;
			sqlx::query_scalar(
				&reinhardt::query::Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(CAST(? AS TIMESTAMPTZ) > CLOCK_TIMESTAMP())".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **access.tx)
			.await?
		};
		if !live {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	.await;
	if let Err(error) = result {
		return access.finish(Err(error)).await;
	}
	Ok((access, description))
}

/// A Home callback checks local authority only. It must never recursively call
/// Home while Home holds source leases and waits for this receiver.
pub(crate) async fn leaf_lease(
	f: &Federation,
	source: &str,
	grant: Uuid,
	admission: Uuid,
) -> Result<(Access, Description)> {
	let record: Record = {
		let query_bind_1 = admission;
		let query_bind_2 = source;
		let query_bind_3 = grant;
		sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=? AND source_node=? AND grant_id=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&f.store.pool)
		.await?
	}
	.ok_or(Error::Forbidden)?;
	let description: Description = serde_json::from_value(record.description.clone())?;
	if description.source_node != source
		|| description.target_node != f.config.node_id
		|| description.grant_id != grant
	{
		return Err(Error::Forbidden);
	}
	let (mut access, description) = receiver_lease(f, source, description).await?;
	let result = async {
		if !record.matches(&access, &description)? {
			return Err(Error::Forbidden);
		}
		let run = f.store.run(admission).await?;
		if run.home_node != source
			|| run.task_id != record.task_id
			|| run.control != crate::domain::RunControl::Active
			|| run.agent_id != description.inspection.agent.id
			|| run.agent_version != description.inspection.agent.version
		{
			return Err(Error::Forbidden);
		}
		crate::generation::foreign::require_active(&mut access, &description, admission).await?;
		access.worker();
		Ok(())
	}
	.await;
	match result {
		Ok(()) => Ok((access, description)),
		Err(error) => access.finish(Err(error)).await,
	}
}

use reinhardt::query::ColumnRef::Asterisk;

impl Record {
	fn view(&self, description: &Description) -> Admission {
		Admission {
			id: self.id,
			source_node: self.source_node.clone(),
			grant_id: self.grant_id,
			task_id: self.task_id,
			expires_at: description.expires_at,
		}
	}
}
use reinhardt::query::SimpleExpr;
