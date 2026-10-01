use super::{accounting, contracts::*, evidence, extraction, network, persistence};
use crate::{
	Error, Result,
	authorization::{
		access::Access,
		execution,
		identity::SubjectIdentity,
		policy::{Resource, SubjectKind},
	},
	capabilities::{
		records::{self, Record},
		service as core,
	},
	domain::Run,
	store::Store,
	web_search::{BraveClient, ValidatedSearch},
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use std::time::Duration as Wait;
use uuid::Uuid;

pub(crate) enum Prepared {
	Output(Value),
	Dispatch(Uuid),
}
pub(crate) fn active(run: &Run) -> bool {
	!matches!(run.phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED")
		&& run.control != "CANCELLED"
}
pub(crate) async fn authorize(
	store: &Store,
	access: &mut Access,
	run: &Run,
	name: &str,
) -> Result<()> {
	if !store.web.profile.admission || run.home_node != store.node_id {
		return Err(Error::Forbidden);
	}
	store.web.profile.validate()?;
	execution::inherit_run_authority(access, run).await?;
	if !access.run_visible(run).await? {
		return Err(Error::NotFound("run unavailable".into()));
	}
	let config = core::settings(access, run).await?;
	if !config.core_capabilities.permits(name) {
		return Err(Error::Forbidden);
	}
	access
		.require(
			&access.resource("tool", format!("builtin:{name}"), json!({})),
			"tool.invoke",
		)
		.await
}
pub(crate) async fn attach_response(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	run: &Run,
	message: &crate::domain::Message,
) -> Result<()> {
	use sea_orm::sea_query::{Alias, Expr, OnConflict, PostgresQueryBuilder, Query};
	let thread: Option<String> = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("data->>'thread_id'"))
			.from(Alias::new("web_runs"))
			.and_where(Expr::cust("run_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_optional(&mut **tx)
	.await?
	.flatten();
	if let Some(thread) = thread {
		let thread: Uuid = thread.parse().map_err(|_| Error::Forbidden)?;
		crate::capabilities::thread_lifecycle::visible(tx, thread).await?;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("channel_message_context"))
				.columns(
					[
						"message_id",
						"workspace_id",
						"thread_id",
						"attachment_digest",
					]
					.map(Alias::new),
				)
				.values_panic((1..=4).map(|i| Expr::cust(format!("${i}"))))
				.on_conflict(
					OnConflict::column(Alias::new("message_id"))
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(message.id)
		.bind(run.workspace_id)
		.bind(thread)
		.bind(crate::registry::digest(&json!([])))
		.execute(&mut **tx)
		.await?;
	}
	Ok(())
}
fn disclosure_resource(access: &Access, run: &Run, target: &Value) -> Resource {
	access.resource("outbound", target["destination"].as_str().unwrap_or("brave"),
		json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"request_digest":crate::registry::digest(target)}))
}
fn eligible(access: &Access, subject: &str, requester: &str, target: &Resource) -> bool {
	let mut evaluation = access.evaluation(subject, target, "web.disclose");
	evaluation.environment["transport"] = json!("api");
	access
		.snapshot
		.bundle
		.subjects
		.get(subject)
		.is_some_and(|s| {
			s.enabled
				&& s.kind == SubjectKind::User
				&& (subject == requester || s.attributes["web_approver"] == true)
		}) && access.snapshot.bundle.evaluate(&evaluation).allowed
}
fn approver(access: &Access, target: &Resource) -> Option<String> {
	if eligible(
		access,
		&access.identity.subject,
		&access.identity.subject,
		target,
	) {
		return Some(access.identity.subject.clone());
	}
	access
		.snapshot
		.bundle
		.subjects
		.keys()
		.find(|subject| eligible(access, subject, &access.identity.subject, target))
		.cloned()
}
fn disclose_not_denied(access: &Access, target: &Resource) -> Result<()> {
	for subject in &access.subjects {
		let decision =
			access
				.snapshot
				.bundle
				.evaluate(&access.evaluation(subject, target, "web.disclose"));
		if !decision.allowed && decision.reason != "no_matching_allow" {
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}
pub(crate) async fn url_intent(
	access: &mut Access,
	run: &Run,
	url: &str,
	message: Uuid,
	request: Uuid,
) -> Result<()> {
	use sea_orm::sea_query::{Alias, Expr, OnConflict, PostgresQueryBuilder, Query};
	let target = disclosure_resource(access, run, &json!({"destination":url,"method":"GET"}));
	if !eligible(
		access,
		&access.identity.subject,
		&access.identity.subject,
		&target,
	) {
		return Err(Error::Forbidden);
	}
	disclose_not_denied(access, &target)?;
	access.require(&target, "network.get").await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("authorization_run_reads"))
			.columns(["run_id", "workspace_id", "resource_kind", "resource_id"].map(Alias::new))
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::val("message").into(),
				Expr::cust("$3"),
			])
			.on_conflict(OnConflict::new().do_nothing().to_owned())
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(run.workspace_id)
	.bind(message)
	.execute(&mut **access.tx)
	.await?;
	records::insert(access,Uuid::new_v4(),None,"web.intent","ready",json!({"run_id":run.id,
		"agent_id":run.agent_id,"agent_version":run.agent_version,"url":url,"message_id":message,"request_action_id":request,
		"decided_by":access.identity.subject}),Some(Utc::now()+Duration::minutes(15))).await?;
	Ok(())
}
async fn use_url_intent(
	access: &mut Access,
	run: &Run,
	operation: &mut Record,
	target: &Resource,
) -> Result<bool> {
	use sea_orm::sea_query::{Alias, Expr, LockType, PostgresQueryBuilder};
	let candidates:Vec<Record>=sqlx::query_as(&persistence::select("core_records")
		.and_where(Expr::cust("tenant=$1 AND kind='web.intent' AND state='ready' AND data->>'run_id'=$2 AND data->>'url'=$3 AND expires_at>CURRENT_TIMESTAMP"))
		.order_by(Alias::new("id"),sea_orm::sea_query::Order::Asc).lock(LockType::Update).limit(1).to_string(PostgresQueryBuilder))
		.bind(&access.identity.tenant).bind(run.id.to_string()).bind(operation.data["target"]["destination"].as_str().ok_or(Error::Forbidden)?)
		.fetch_all(&mut **access.tx).await?;
	let Some(mut intent) = candidates.into_iter().next() else {
		return Ok(false);
	};
	if intent.data["agent_id"] != run.agent_id
		|| intent.data["agent_version"] != run.agent_version
		|| !eligible(access, &intent.owner, &operation.owner, target)
	{
		return Ok(false);
	}
	let message: Uuid = serde_json::from_value(intent.data["message_id"].clone())?;
	access
		.workspace_record(run.workspace_id, "message", message)
		.await?;
	intent.state = "consumed".into();
	intent.data["operation_id"] = json!(operation.id);
	records::update(access, &mut intent).await?;
	operation.data["approver"] = json!(intent.owner);
	operation.data["decided_by"] = intent.data["decided_by"].clone();
	operation.data["disclosure_required"] = json!(true);
	operation.data["intent_id"] = json!(intent.id);
	operation.expires_at = intent.expires_at;
	records::update(access, operation).await?;
	Ok(true)
}
fn reject_known_secrets(store: &Store, input: &Value) -> Result<()> {
	let text = input.to_string();
	let references = store
		.web
		.account
		.iter()
		.map(|account| account.credential_env.as_str())
		.chain(
			store
				.capabilities
				.0
				.runner
				.iter()
				.map(|runner| runner.token_env.as_str()),
		);
	for reference in references {
		if let Ok(secret) = crate::config::secret(reference)
			&& !secret.is_empty()
			&& text.contains(&secret)
		{
			return Err(Error::Invalid("secret_disclosure_denied".into()));
		}
	}
	Ok(())
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	name: &str,
	input: Value,
	key: &str,
) -> Result<Prepared> {
	authorize(store, access, run, name).await?;
	if !active(run) {
		return Ok(Prepared::Output(failure(name, "cancelled", "run_stopped")));
	}
	if key.len() > 1024 || serde_json::to_vec(&input)?.len() > 4096 {
		return Err(Error::Invalid("web input exceeds limit".into()));
	}
	reject_known_secrets(store, &input)?;
	let mut state = persistence::state(access, run).await?;
	if state["revoked"] == true {
		return Err(Error::Forbidden);
	}
	let digest = crate::registry::digest(&json!([
		"web-invocation/1",
		run.id,
		run.agent_id,
		run.agent_version,
		name,
		input
	]));
	if let Some(mut record) = persistence::existing(access, run, key).await? {
		if record.data["input_digest"] != digest {
			return Err(Error::Conflict("web invocation input changed".into()));
		}
		if record.state == "completed" {
			evidence::validate_result(access, run, &record.data["result"]).await?;
			return Ok(Prepared::Output(record.data["result"].clone()));
		}
		if record.state == "dispatched" || record.state == "uncertain" {
			record.state = "uncertain".into();
			records::update(access, &mut record).await?;
			return Ok(Prepared::Output(failure(
				name,
				"uncertain",
				"dispatch_outcome_unknown",
			)));
		}
		if record.state == "result_recorded" {
			return resume_response(store, access, run, record).await;
		}
		if record.state == "denied" {
			return Ok(Prepared::Output(failure(
				name,
				"error",
				"disclosure_denied",
			)));
		}
		if record.state == "pending" {
			let approval = records::get(access, record.id, "web.operation").await?;
			if persistence::expired(&approval) || approval.data["denied"] == true {
				return Ok(Prepared::Output(failure(
					name,
					"error",
					"disclosure_denied_or_expired",
				)));
			}
			return Ok(Prepared::Output(approval_view(&approval)));
		}
		if record.state == "approved" || record.state == "prepared" {
			record.data["worker"] = json!(run.lease_owner.ok_or(Error::Forbidden)?);
			records::update(access, &mut record).await?;
			return Ok(Prepared::Dispatch(record.id));
		}
		return Ok(Prepared::Output(failure(
			name,
			"error",
			"operation_unavailable",
		)));
	}
	let context_digest = persistence::fingerprint(access, run).await?;
	let mut target = json!({"destination":null});
	let mut resolved = input.clone();
	let mut network_required = false;
	match name {
		"web_search" => {
			if !store.web.search_available() {
				return Ok(Prepared::Output(failure(
					name,
					"error",
					"account_unavailable",
				)));
			}
			let entry = crate::authorization::catalog::entry(
				access,
				&crate::registry::EntityRef {
					id: run.agent_id.clone(),
					version: run.agent_version.clone(),
				},
				"agent.execute",
			)
			.await?;
			let preferred = entry
				.languages
				.iter()
				.find_map(|language| match language.as_str() {
					"ja" => Some(crate::web_search::Language::Ja),
					"en" => Some(crate::web_search::Language::En),
					_ => None,
				});
			let search = ValidatedSearch::constrained(
				input.clone(),
				preferred,
				&store.web.profile.allowed_domains,
				&store.web.profile.denied_domains,
			)?;
			resolved["preferred_language"] = json!(preferred);
			target = json!({"destination":"https://api.search.brave.com","request":search.provider_body(),
				"account_id":store.web.account.as_ref().ok_or(Error::Forbidden)?.account_id,
				"account_digest":crate::registry::digest(&serde_json::to_value(store.web.account.as_ref().ok_or(Error::Forbidden)?.as_ref())?)});
			network_required = true;
			let account = store.web.account.as_ref().ok_or(Error::Forbidden)?;
			let until = Utc::now() + Duration::days(i64::from(account.retention_days.min(90)));
			if persistence::date(&state["retention_until"])? > until {
				state["retention_until"] = json!(until);
				persistence::save_state(access, run, &state).await?;
			}
		}
		"web_open" => {
			let open: Open = serde_json::from_value(input.clone())
				.map_err(|_| Error::Invalid("invalid web_open input".into()))?;
			open.validate()?;
			if let Some(document) = open.document_id {
				persistence::owned(access, run, document, "web.document").await?;
			} else {
				if store.capabilities.0.runner.is_none() {
					return Ok(Prepared::Output(failure(
						name,
						"error",
						"web_extractor_unavailable",
					)));
				}
				let url = if let Some(source) = open.source_id {
					let source = persistence::owned(access, run, source, "web.source").await?;
					source.data["url"]
						.as_str()
						.ok_or(Error::Forbidden)?
						.to_owned()
				} else {
					open.url.ok_or(Error::Forbidden)?
				};
				network::url(&url)?;
				let url = network::url(&url)?.to_string();
				if !store.web.permits_url(&url) {
					return Err(Error::Invalid("operator_domain_denied".into()));
				}
				resolved["resolved_url"] = json!(url);
				target = json!({"destination":url,"method":"GET"});
				network_required = true;
			}
		}
		"web_find" => {
			let find: Find = serde_json::from_value(input.clone())
				.map_err(|_| Error::Invalid("invalid web_find input".into()))?;
			find.validate()?;
			persistence::owned(access, run, find.document_id, "web.document").await?;
		}
		_ => return Err(Error::Forbidden),
	}
	let mut record = persistence::insert_operation(access, run, key, json!({
		"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"workspace_id":run.workspace_id,
		"credential_id":access.identity.credential_id,"subject_chain":access.subjects,"name":name,"input":input,
		"resolved":resolved,"input_digest":digest,"context_digest":context_digest,"target":target,
		"request_digest":crate::registry::digest(&json!([digest,context_digest,target])),"attempts":0,"retries":0,"redirects":0
		,"worker":run.lease_owner
	})).await?;
	if !network_required {
		let result = tokio::time::timeout(Wait::from_secs(2), async {
			if name == "web_open" {
				evidence::open(access, run, serde_json::from_value(input)?).await
			} else {
				evidence::find(access, run, serde_json::from_value(input)?).await
			}
		})
		.await
		.map_err(|_| Error::Invalid("local_deadline_exceeded".into()))??;
		record.state = "completed".into();
		record.data["result"] = result.clone();
		records::update(access, &mut record).await?;
		return Ok(Prepared::Output(result));
	}
	let resource = disclosure_resource(access, run, &record.data["target"]);
	access.require(&resource, "network.get").await?;
	disclose_not_denied(access, &resource)?;
	if name == "web_open" && use_url_intent(access, run, &mut record, &resource).await? {
		return Ok(Prepared::Dispatch(record.id));
	}
	let public = state["classification"] == "public" && state["context_digest"] == context_digest;
	if public {
		access.require(&resource, "web.disclose").await?;
	}
	if !public {
		state["classification"] = json!("unclassified");
		persistence::save_state(access, run, &state).await?;
		record.state = "pending".into();
		record.expires_at = Some(Utc::now() + Duration::minutes(15));
		record.data["approver"] = json!(approver(access, &resource).ok_or(Error::Forbidden)?);
		record.data["disclosure_required"] = json!(true);
		records::update(access, &mut record).await?;
		store
			.event(
				&mut access.tx,
				Some(run.workspace_id),
				"web.disclosure_requested",
				json!({"run_id":run.id,"approval_id":record.id}),
			)
			.await?;
		return Ok(Prepared::Output(approval_view(&record)));
	}
	record.data["public_context"] = json!(true);
	records::update(access, &mut record).await?;
	Ok(Prepared::Dispatch(record.id))
}
pub(crate) fn approval_view(record: &Record) -> Value {
	let mut value = envelope(
		record.data["name"].as_str().unwrap_or("web_open"),
		"approval_required",
		json!({
		"approval_id":record.id,"revision":record.revision,"request_digest":record.data["request_digest"],
		"target":record.data["target"],"approver":record.data["approver"],"expires_at":record.expires_at}),
	);
	value["approval_id"] = json!(record.id);
	value
}
pub(crate) async fn decide(
	store: &Store,
	access: &mut Access,
	run: &Run,
	id: Uuid,
	input: DisclosureDecision,
) -> Result<Value> {
	if !active(run) || !access.run_visible(run).await? {
		return Err(Error::NotFound("approval unavailable".into()));
	}
	let mut record = persistence::owned(access, run, id, "web.operation").await?;
	let target = disclosure_resource(access, run, &record.data["target"]);
	if record.data["approver"] != access.identity.subject
		|| !eligible(access, &access.identity.subject, &record.owner, &target)
	{
		return Err(Error::Forbidden);
	}
	if record.revision != input.expected_revision
		|| record.data["request_digest"] != input.request_digest
		|| record.state != "pending"
		|| persistence::expired(&record)
		|| record.data["context_digest"] != persistence::fingerprint(access, run).await?
	{
		return Err(Error::Conflict("disclosure_stale_or_expired".into()));
	}
	let previous = access.subjects.clone();
	access.subjects = serde_json::from_value(record.data["subject_chain"].clone())?;
	disclose_not_denied(access, &target)?;
	access.require(&target, "network.get").await?;
	access.subjects = previous;
	record.state = if input.allow_once {
		"approved"
	} else {
		"denied"
	}
	.into();
	record.data["denied"] = json!(!input.allow_once);
	record.data["decided_by"] = json!(access.identity.subject);
	record.data["decided_at"] = json!(Utc::now());
	record.data["decision_revision"] = json!(access.snapshot.revision);
	record.data["deadline"] = Value::Null;
	records::update(access, &mut record).await?;
	store
		.event(
			&mut access.tx,
			Some(run.workspace_id),
			"web.disclosure_decided",
			json!({"run_id":run.id,"approval_id":id,"allowed":input.allow_once}),
		)
		.await?;
	Ok(json!({"approval_id":id,"state":record.state,"revision":record.revision}))
}

async fn dispatch_access(store: &Store, snapshot: &Record) -> Result<(Access, Run)> {
	let identity = SubjectIdentity {
		credential_id: serde_json::from_value(snapshot.data["credential_id"].clone())?,
		tenant: snapshot.tenant.clone(),
		subject: snapshot.owner.clone(),
	};
	let mut access = Access::begin(store, &identity).await?;
	let run = store
		.run(serde_json::from_value(snapshot.data["run_id"].clone())?)
		.await?;
	authorize(
		store,
		&mut access,
		&run,
		snapshot.data["name"].as_str().ok_or(Error::Forbidden)?,
	)
	.await?;
	// All Web writers acquire the Run accounting row before operation/document
	// rows. Expiry and dispatch therefore use the same lock order.
	let state = persistence::state(&mut access, &run).await?;
	if state["revoked"] == true {
		return Err(Error::Forbidden);
	}
	if !active(&run) || run.control == "PAUSED" {
		return Err(Error::Forbidden);
	}
	// Fence the exact Worker owning this invocation, including credential rotation.
	if let Some(worker) = snapshot.data.get("worker") {
		let worker: Uuid = serde_json::from_value(worker.clone()).map_err(|_| Error::Forbidden)?;
		let live:Option<Uuid>=sqlx::query_scalar(&persistence::select("runs").clear_selects().column(sea_orm::sea_query::Alias::new("id"))
			.and_where(sea_orm::sea_query::Expr::cust("id=$1 AND lease_owner=$2 AND lease_until>CURRENT_TIMESTAMP AND control NOT IN ('CANCELLED','PAUSED')"))
			.lock(sea_orm::sea_query::LockType::Share).to_string(sea_orm::sea_query::PostgresQueryBuilder))
			.bind(run.id).bind(worker).fetch_optional(&mut **access.tx).await?;
		if live.is_none() {
			return Err(Error::Forbidden);
		}
	}
	Ok((access, run))
}
async fn check_disclosure(access: &mut Access, run: &Run, record: &Record) -> Result<()> {
	let target = disclosure_resource(access, run, &record.data["target"]);
	access.require(&target, "network.get").await?;
	disclose_not_denied(access, &target)?;
	if record.data["context_digest"] != persistence::fingerprint(access, run).await? {
		return Err(Error::Invalid("disclosure_context_changed".into()));
	}
	if record.data["disclosure_required"] == true {
		let approver = record.data["approver"].as_str().ok_or(Error::Forbidden)?;
		if persistence::expired(record)
			|| !eligible(access, approver, &record.owner, &target)
			|| record.data["decided_by"].is_null()
		{
			return Err(Error::Invalid("disclosure_expired_or_revoked".into()));
		}
		if let Some(intent) = record.data["intent_id"].as_str() {
			let intent = persistence::owned(
				access,
				run,
				intent.parse().map_err(|_| Error::Forbidden)?,
				"web.intent",
			)
			.await?;
			access
				.workspace_record(
					run.workspace_id,
					"message",
					serde_json::from_value(intent.data["message_id"].clone())?,
				)
				.await?;
		}
	} else {
		let state = persistence::state(access, run).await?;
		if state["classification"] != "public"
			|| state["context_digest"] != record.data["context_digest"]
		{
			return Err(Error::Invalid("disclosure_context_changed".into()));
		}
		access.require(&target, "web.disclose").await?;
	}
	Ok(())
}
fn check_account(store: &Store, record: &Record) -> Result<()> {
	if record.data["name"] == "web_search" {
		let account = store
			.web
			.account
			.as_ref()
			.ok_or_else(|| Error::Invalid("account_unavailable".into()))?;
		account.validate_at(Utc::now())?;
		if record.data["target"]["account_digest"]
			!= crate::registry::digest(&serde_json::to_value(account.as_ref())?)
		{
			return Err(Error::Invalid("disclosure_account_changed".into()));
		}
	}
	Ok(())
}
async fn withdrawn(store: &Store, snapshot: &Record) -> Result<()> {
	loop {
		let (mut access, run) = dispatch_access(store, snapshot).await?;
		let record = persistence::owned(&mut access, &run, snapshot.id, "web.operation").await?;
		let checked = check_disclosure(&mut access, &run, &record).await;
		access.finish(checked).await?;
		tokio::time::sleep(Wait::from_millis(200)).await;
	}
}
pub(crate) async fn drive(store: &Store, id: Uuid) -> Result<Value> {
	let snapshot: Record = sqlx::query_as(
		&persistence::select("core_records")
			.and_where(sea_orm::sea_query::Expr::cust(
				"id=$1 AND kind='web.operation'",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_one(&store.pool)
	.await?;
	let name = snapshot.data["name"].as_str().ok_or(Error::Forbidden)?;
	let duration = if name == "web_search" { 15 } else { 20 };
	let started = Utc::now();
	let deadline = if snapshot.data["deadline"].is_null() {
		started
			+ Duration::milliseconds(
				snapshot.data["remaining_ms"]
					.as_i64()
					.unwrap_or(duration * 1000)
					.min(duration * 1000),
			)
	} else {
		persistence::date(&snapshot.data["deadline"])?
	};
	let remaining = (deadline - started).to_std().unwrap_or_default();
	// Keep the independent dispatch/withdrawal state machines on the heap;
	// embedding both SQL + HTTP futures in every Harness step exceeds small
	// Worker thread stacks in debug builds.
	let operation = Box::pin(drive_until(store, &snapshot, deadline));
	let withdrawal = Box::pin(withdrawn(store, &snapshot));
	let result = tokio::select! {
		result=tokio::time::timeout(remaining,operation)=>match result {Ok(result)=>result,Err(_)=>Ok(failure(name,"uncertain","deadline_exceeded"))},
		_=withdrawal=>Ok(failure(name,"cancelled","authority_withdrawn"))
	};
	accounting::release(store, id).await?;
	if result
		.as_ref()
		.is_ok_and(|value| matches!(value["status"].as_str(), Some("uncertain" | "cancelled")))
	{
		// A timeout or cancellation after possible dispatch never makes the
		// operation replayable, even if the generic invocation journal recovers.
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("core_records"))
				.value(sea_orm::sea_query::Alias::new("state"), "uncertain")
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision+1"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id=$1 AND kind='web.operation' AND state='dispatched'",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.execute(&store.pool)
		.await?;
	}
	result
}
async fn drive_until(
	store: &Store,
	snapshot: &Record,
	deadline: chrono::DateTime<Utc>,
) -> Result<Value> {
	let name = snapshot.data["name"].as_str().ok_or(Error::Forbidden)?;
	loop {
		let (mut access, run) = dispatch_access(store, snapshot).await?;
		let mut record =
			persistence::owned(&mut access, &run, snapshot.id, "web.operation").await?;
		if record.data["deadline"].is_null() {
			record.data["deadline"] = json!(deadline);
			records::update(&mut access, &mut record).await?;
		}
		check_disclosure(&mut access, &run, &record).await?;
		check_account(store, &record)?;
		if name == "web_open"
			&& !store.web.permits_url(
				record.data["target"]["destination"]
					.as_str()
					.ok_or(Error::Forbidden)?,
			) {
			return Err(Error::Invalid("operator_domain_denied".into()));
		}
		if matches!(record.state.as_str(), "dispatched" | "uncertain") {
			return Ok(failure(name, "uncertain", "dispatch_outcome_unknown"));
		}
		if record.state == "result_recorded" {
			let prepared = resume_response(store, &mut access, &run, record).await?;
			if let Prepared::Output(result) = prepared {
				return access.finish(Ok(result)).await;
			}
			access.finish(Ok(())).await?;
			continue;
		}
		if let Some(until) = record.data["retry_not_before"].as_str() {
			let until = chrono::DateTime::parse_from_rfc3339(until)
				.map_err(|_| Error::Forbidden)?
				.with_timezone(&Utc);
			if until > Utc::now() {
				access.finish(Ok(())).await?;
				tokio::time::sleep((until - Utc::now()).to_std().unwrap_or_default()).await;
				continue;
			}
		}
		let search = name == "web_search";
		accounting::check_attempt_limit(
			store,
			&persistence::state(&mut access, &run).await?,
			search,
		)?;
		let search_request = if search {
			let request = ValidatedSearch::constrained(
				record.data["input"].clone(),
				serde_json::from_value(record.data["resolved"]["preferred_language"].clone())?,
				&store.web.profile.allowed_domains,
				&store.web.profile.denied_domains,
			)?;
			if request.provider_body() != record.data["target"]["request"] {
				return Err(Error::Invalid("disclosure_request_changed".into()));
			}
			Some(request)
		} else {
			None
		};
		let destination = record.data["target"]["destination"]
			.as_str()
			.ok_or(Error::Forbidden)?
			.to_owned();
		let host = network::url(&destination)?
			.host_str()
			.ok_or(Error::Forbidden)?
			.to_owned();
		let page_request = if search {
			None
		} else {
			Some(
				network::prepare(
					&destination,
					(deadline - Utc::now())
						.to_std()
						.map_err(|_| Error::Invalid("deadline_exceeded".into()))?,
				)
				.await?,
			)
		};
		if !accounting::reserve(store, &mut access, &run, record.id, search, &host, deadline)
			.await?
		{
			access.finish(Ok(())).await?;
			tokio::time::sleep(Wait::from_millis(100)).await;
			continue;
		}
		record.state = "dispatched".into();
		record.data["deadline"] = json!(deadline);
		record.data["attempts"] = json!(record.data["attempts"].as_u64().unwrap_or(0) + 1);
		record.data["worker"] = json!(run.lease_owner.ok_or(Error::Forbidden)?);
		records::update(&mut access, &mut record).await?;
		access.finish(Ok(())).await?;
		let response = if search {
			BraveClient::new()?
				.search(
					search_request.as_ref().ok_or(Error::Forbidden)?,
					store.web.account.as_ref().ok_or(Error::Forbidden)?,
				)
				.await?
		} else {
			match network::fetch(page_request.ok_or(Error::Forbidden)?).await {
				Ok(network::Page::Redirect(url)) => envelope(name, "redirect", json!({"url":url})),
				Ok(network::Page::Retry(status, retry)) => {
					let mut value = failure(name, "error", "page_unavailable");
					value["error"]["retryable"] = json!(true);
					value["error"]["http_status"] = json!(status);
					value["error"]["retry_after"] = json!(retry);
					value
				}
				Ok(network::Page::Error(code)) => failure(name, "error", code),
				Ok(network::Page::Body {
					bytes,
					media_type,
					encoding,
				}) => {
					let fetched_at = Utc::now();
					match extraction::extract(store, &bytes, &media_type, &encoding).await {
						Ok(extracted) => envelope(
							name,
							"extracted",
							json!({"extraction":extracted,"raw_digest":crate::capabilities::objects::digest(&bytes),"media_type":media_type,"url":destination,"fetched_at":fetched_at}),
						),
						Err(_) => failure(name, "error", "extraction_failed"),
					}
				}
				Err(Error::Invalid(code)) => failure(name, "error", &code),
				Err(_) => failure(name, "uncertain", "page_transport_uncertain"),
			}
		};
		accounting::release(store, record.id).await?;
		// Persist a response before completing the immutable invocation. Recovery
		// finalizes this state locally and never repeats the external HTTP effect.
		let (mut access, run) = dispatch_access(store, &record).await?;
		let mut current = persistence::owned(&mut access, &run, record.id, "web.operation").await?;
		check_disclosure(&mut access, &run, &current).await?;
		current.data["response"] = response.clone();
		current.state = "result_recorded".into();
		records::update(&mut access, &mut current).await?;
		access.finish(Ok(())).await?;
		let (mut access, run) = dispatch_access(store, &current).await?;
		let current = persistence::owned(&mut access, &run, current.id, "web.operation").await?;
		match resume_response(store, &mut access, &run, current).await? {
			Prepared::Output(result) => return access.finish(Ok(result)).await,
			Prepared::Dispatch(_) => {
				access.finish(Ok(())).await?;
				continue;
			}
		}
	}
}
fn retry_delay(value: &Value) -> Wait {
	if let Some(value) = value.as_str() {
		if let Ok(seconds) = value.parse::<u64>() {
			return Wait::from_secs(seconds.min(86400));
		}
		if let Ok(date) = chrono::DateTime::parse_from_rfc2822(value) {
			return (date.with_timezone(&Utc) - Utc::now())
				.to_std()
				.unwrap_or_default();
		}
	}
	Wait::from_millis(200)
}
async fn resume_response(
	store: &Store,
	access: &mut Access,
	run: &Run,
	mut record: Record,
) -> Result<Prepared> {
	let response = record.data["response"].clone();
	let name = record.data["name"]
		.as_str()
		.ok_or(Error::Forbidden)?
		.to_owned();
	check_disclosure(access, run, &record).await?;
	if response["status"] == "redirect" {
		let redirects = record.data["redirects"].as_u64().unwrap_or(0);
		if redirects >= 3 {
			record.data["response"] = failure(&name, "error", "redirect_limit");
			return finish_response(store, access, run, record).await;
		}
		let url = network::url(response["data"]["url"].as_str().ok_or(Error::Forbidden)?)?;
		if !store.web.permits_url(url.as_str()) {
			record.data["response"] = failure(&name, "error", "operator_domain_denied");
			return finish_response(store, access, run, record).await;
		}
		record.data["redirects"] = json!(redirects + 1);
		record.data["resolved"]["resolved_url"] = json!(url.as_str());
		record.data["target"] = json!({"destination":url.as_str(),"method":"GET"});
		record.data["request_digest"] = json!(crate::registry::digest(&json!([
			record.data["input_digest"],
			record.data["context_digest"],
			record.data["target"]
		])));
		let target = disclosure_resource(access, run, &record.data["target"]);
		access.require(&target, "network.get").await?;
		disclose_not_denied(access, &target)?;
		record.data["approver"] = json!(approver(access, &target).ok_or(Error::Forbidden)?);
		record.data["remaining_ms"] = json!(
			(persistence::date(&record.data["deadline"])? - Utc::now())
				.num_milliseconds()
				.max(0)
		);
		record.data["deadline"] = Value::Null;
		record.data["response"] = Value::Null;
		record.data["decided_by"] = Value::Null;
		record.state = "pending".into();
		record.data["disclosure_required"] = json!(true);
		record.expires_at = Some(Utc::now() + Duration::minutes(15));
		records::update(access, &mut record).await?;
		return Ok(Prepared::Output(approval_view(&record)));
	}
	if response["error"]["retryable"] == true && record.data["retries"].as_u64().unwrap_or(0) == 0 {
		let until = Utc::now()
			+ Duration::from_std(retry_delay(&response["error"]["retry_after"]))
				.unwrap_or(Duration::MAX);
		if until < persistence::date(&record.data["deadline"])? {
			record.data["retries"] = json!(1);
			record.data["retry_not_before"] = json!(until);
			record.data["response"] = Value::Null;
			record.state = "prepared".into();
			record.data["worker"] = json!(run.lease_owner.ok_or(Error::Forbidden)?);
			records::update(access, &mut record).await?;
			return Ok(Prepared::Dispatch(record.id));
		}
	}
	finish_response(store, access, run, record).await
}
async fn finish_response(
	store: &Store,
	access: &mut Access,
	run: &Run,
	mut record: Record,
) -> Result<Prepared> {
	let name = record.data["name"]
		.as_str()
		.ok_or(Error::Forbidden)?
		.to_owned();
	let mut result = record.data["response"].clone();
	if name == "web_search" && matches!(result["status"].as_str(), Some("ok" | "empty")) {
		evidence::sources(access, run, &mut result).await?;
	} else if result["status"] == "extracted" {
		result = evidence::document(access, run, &record.data["input"], &result["data"]).await?;
	}
	if result.is_null() {
		result = failure(&name, "uncertain", "dispatch_outcome_unknown");
	}
	if result.to_string().len() > MAX_ENVELOPE {
		result = failure(&name, "error", "response_too_large");
	}
	record.state = "completed".into();
	record.data["result"] = result.clone();
	record.data["response"] = Value::Null;
	records::update(access, &mut record).await?;
	store
		.event(
			&mut access.tx,
			Some(run.workspace_id),
			"web.operation_completed",
			json!({"run_id":run.id,"operation_id":record.id,"status":result["status"]}),
		)
		.await?;
	Ok(Prepared::Output(result))
}
