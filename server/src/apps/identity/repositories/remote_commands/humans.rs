//! Bounded Home-owned continuations live under the exact remote execution binding.
use crate::{Error, Result};
use aidash_domain::HumanRequest;
use reinhardt::{
	db::{backends::TransactionExecutor, orm::execution::convert_values},
	query::{Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder},
};
use serde_json::Value;
use uuid::Uuid;

struct Journal {
	admission: Uuid,
	requests: Vec<Record>,
	ended: bool,
}

async fn records(tx: &mut dyn TransactionExecutor, grant: Uuid, lock: bool) -> Result<Journal> {
	let mut query = Query::select();
	query
		.columns([Alias::new("human_requests"), Alias::new("admission_id")])
		.from(Alias::new("authorization_remote_execution"))
		.and_where(Expr::col("grant_id").eq(Expr::value(grant)));
	if lock {
		query.lock(LockType::Update);
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	let row = tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.ok_or(Error::Forbidden)?;
	let row = reinhardt::db::orm::QueryRow::from_backend_row(row);
	let admission = row.get("admission_id").ok_or(Error::Forbidden)?;
	let entries = row.data["human_requests"]
		.as_array()
		.ok_or_else(|| Error::Invalid("invalid Home human-request journal".into()))?;
	let mut requests = Vec::new();
	let mut ended = false;
	for entry in entries {
		if entry == &serde_json::json!({"run_ended": true}) {
			ended = true;
		} else {
			requests.push(serde_json::from_value(entry.clone())?);
		}
	}
	Ok(Journal {
		admission,
		requests,
		ended,
	})
}
async fn save(tx: &mut dyn TransactionExecutor, grant: Uuid, journal: &Journal) -> Result<()> {
	let mut entries = serde_json::to_value(&journal.requests)?;
	if journal.ended {
		entries
			.as_array_mut()
			.expect("serialized requests are an array")
			.push(serde_json::json!({"run_ended": true}));
	}
	let content = entries;
	// A closing marker must fit even when the existing journal is at its limit.
	let limit = 1_048_576 + if journal.ended { 32_768 } else { 0 };
	if serde_json::to_vec(&content)?.len() > limit {
		return Err(Error::Invalid(
			"Home human-request journal exceeds 1 MiB".into(),
		));
	}
	let (sql, values) = Query::update()
		.table(Alias::new("authorization_remote_execution"))
		.value(Alias::new("human_requests"), content)
		.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}
pub(crate) async fn create_in(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	admission: Uuid,
	workspace: Uuid,
	kind: &str,
	prompt: &str,
	key: &str,
) -> Result<HumanRequest> {
	crate::apps::execution::services::human_interaction::validate_request(kind, prompt)?;
	if prompt.len() > 64_000 {
		return Err(Error::Invalid("human prompt exceeds 64 KiB".into()));
	}
	let mut journal = records(tx, grant, true).await?;
	if journal.admission != admission {
		return Err(Error::Forbidden);
	}
	if journal.ended {
		return Err(Error::Conflict("run has ended".into()));
	}
	let requests = &mut journal.requests;
	if requests
		.iter()
		.any(|record| record.request.run_id != admission)
	{
		return Err(Error::Forbidden);
	}
	if let Some(previous) = requests.iter().find(|r| r.key == key) {
		let previous = &previous.request;
		if previous.run_id != admission
			|| previous.workspace_id != workspace
			|| previous.kind != kind
			|| previous.prompt != prompt
		{
			return Err(Error::Conflict(
				"human request key binds different input".into(),
			));
		}
		return Ok(previous.clone());
	}
	if requests.len() >= 128 {
		return Err(Error::Invalid(
			"Home human-request count exceeds 128".into(),
		));
	}
	let request = HumanRequest {
		id: Uuid::new_v4(),
		workspace_id: workspace,
		run_id: admission,
		kind: kind.into(),
		prompt: prompt.into(),
		response: None,
		created_at: chrono::Utc::now(),
		answered_by: None,
		answer_by: None,
	};
	requests.push(Record {
		key: key.into(),
		request: request.clone(),
	});
	save(tx, grant, &journal).await?;
	Ok(request)
}
pub(crate) async fn answer_in(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	admission: Uuid,
	id: Uuid,
	response: Value,
	actor: &str,
) -> Result<HumanRequest> {
	crate::apps::execution::services::human_interaction::validate_response(&response)?;
	let mut journal = records(tx, grant, true).await?;
	if journal.admission != admission {
		return Err(Error::Forbidden);
	}
	let record = journal
		.requests
		.iter_mut()
		.find(|r| r.request.id == id && r.request.run_id == admission)
		.ok_or(Error::Forbidden)?;
	let request = &mut record.request;
	if request.answered_by.as_deref() == Some("system")
		&& request
			.response
			.as_ref()
			.and_then(|value| value.get("reason"))
			.and_then(Value::as_str)
			== Some("run_ended")
	{
		return Err(Error::Conflict(
			"human request closed because the run ended".into(),
		));
	}
	if record.key.ends_with(":reconcile") && response.get("result").is_none() {
		return Err(Error::Invalid(
			"tool reconciliation requires a JSON object containing result".into(),
		));
	}
	if let Some(previous) = &request.response {
		if previous != &response {
			return Err(Error::Conflict("human request is already answered".into()));
		}
		return Ok(request.clone());
	}
	if crate::apps::execution::services::human_interaction::approval_expired(
		request,
		chrono::Utc::now(),
	) {
		return Err(Error::Conflict("human approval expired".into()));
	}
	request.response = Some(response);
	request.answered_by = Some(actor.into());
	let answered = request.clone();
	save(tx, grant, &journal).await?;
	Ok(answered)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
	key: String,
	request: HumanRequest,
}
pub(crate) async fn read_in(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	lock: bool,
) -> Result<Vec<HumanRequest>> {
	let mut journal = records(tx, grant, lock).await?;
	let mut changed = false;
	for record in &mut journal.requests {
		let request = &mut record.request;
		if request.response.is_none()
			&& crate::apps::execution::services::human_interaction::approval_expired(
				request,
				chrono::Utc::now(),
			) {
			request.response = Some(serde_json::json!({"approved":false,"expired":true}));
			request.answered_by = Some("system".into());
			changed = true;
		}
	}
	if changed {
		if !lock {
			return read_locked(tx, grant).await;
		}
		save(tx, grant, &journal).await?;
	}
	Ok(journal.requests.into_iter().map(|r| r.request).collect())
}
/// Persist Home's knowledge of the ended execution in the same journal lock
/// that serializes incoming questions and answers.
pub(crate) async fn close_for_run(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	admission: Uuid,
) -> Result<()> {
	let mut journal = records(tx, grant, true).await?;
	if journal.admission != admission
		|| journal
			.requests
			.iter()
			.any(|record| record.request.run_id != admission)
	{
		return Err(Error::Forbidden);
	}
	if journal.ended {
		return Ok(());
	}
	for record in &mut journal.requests {
		if record.request.response.is_none() {
			record.request.response =
				Some(serde_json::json!({"answered":false,"reason":"run_ended"}));
			record.request.answered_by = Some("system".into());
		}
	}
	journal.ended = true;
	save(tx, grant, &journal).await
}

async fn read_locked(tx: &mut dyn TransactionExecutor, grant: Uuid) -> Result<Vec<HumanRequest>> {
	Box::pin(read_in(tx, grant, true)).await
}

pub(crate) async fn create(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	admission: Uuid,
	workspace: Uuid,
	kind: &str,
	prompt: &str,
	key: &str,
) -> Result<HumanRequest> {
	create_in(tx, grant, admission, workspace, kind, prompt, key).await
}
pub(crate) async fn read(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	lock: bool,
) -> Result<Vec<HumanRequest>> {
	read_in(tx, grant, lock).await
}
pub(crate) async fn answer(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	admission: Uuid,
	id: Uuid,
	response: Value,
	actor: &str,
) -> Result<HumanRequest> {
	answer_in(tx, grant, admission, id, response, actor).await
}
