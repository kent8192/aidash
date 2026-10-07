//! Bounded Home-owned continuations live under the exact remote execution binding.
use crate::{Error, Result};
use aidash_domain::HumanRequest;
use reinhardt::{
	db::{backends::TransactionExecutor, orm::execution::convert_values},
	query::{Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder},
};
use serde_json::Value;
use uuid::Uuid;

/// A scoped admission or an operator's exact task delegation owns the journal.
#[derive(Clone, Copy)]
pub(crate) enum Journal {
	Scoped(Uuid),
	Delegation(Uuid),
}
impl Journal {
	fn table(self) -> &'static str {
		match self {
			Self::Scoped(_) => "authorization_remote_execution",
			Self::Delegation(_) => "delegations",
		}
	}
	fn column(self) -> &'static str {
		match self {
			Self::Scoped(_) => "grant_id",
			Self::Delegation(_) => "task_id",
		}
	}
	fn id(self) -> Uuid {
		match self {
			Self::Scoped(id) | Self::Delegation(id) => id,
		}
	}
}

async fn records(
	tx: &mut dyn TransactionExecutor,
	grant: Journal,
	lock: bool,
) -> Result<Vec<Record>> {
	let mut query = Query::select();
	query
		.column(Alias::new("human_requests"))
		.from(Alias::new(grant.table()))
		.and_where(Expr::col(grant.column()).eq(Expr::value(grant.id())));
	if lock {
		query.lock(LockType::Update);
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	let row = tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.ok_or(Error::Forbidden)?;
	let value = reinhardt::db::orm::QueryRow::from_backend_row(row).data["human_requests"].clone();
	Ok(serde_json::from_value(value)?)
}
async fn save(tx: &mut dyn TransactionExecutor, grant: Journal, requests: &[Record]) -> Result<()> {
	let content = serde_json::to_value(requests)?;
	if serde_json::to_vec(&content)?.len() > 1_048_576 {
		return Err(Error::Invalid(
			"Home human-request journal exceeds 1 MiB".into(),
		));
	}
	let (sql, values) = Query::update()
		.table(Alias::new(grant.table()))
		.value(Alias::new("human_requests"), content)
		.and_where(Expr::col(grant.column()).eq(Expr::value(grant.id())))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}
pub(crate) async fn create_in(
	tx: &mut dyn TransactionExecutor,
	grant: Journal,
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
	let mut requests = records(tx, grant, true).await?;
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
	};
	requests.push(Record {
		key: key.into(),
		request: request.clone(),
	});
	save(tx, grant, &requests).await?;
	Ok(request)
}
pub(crate) async fn answer_in(
	tx: &mut dyn TransactionExecutor,
	grant: Journal,
	admission: Uuid,
	id: Uuid,
	response: Value,
	actor: &str,
) -> Result<HumanRequest> {
	crate::apps::execution::services::human_interaction::validate_response(&response)?;
	let mut requests = records(tx, grant, true).await?;
	let record = requests
		.iter_mut()
		.find(|r| r.request.id == id && r.request.run_id == admission)
		.ok_or(Error::Forbidden)?;
	if record.key.ends_with(":reconcile") && response.get("result").is_none() {
		return Err(Error::Invalid(
			"tool reconciliation requires a JSON object containing result".into(),
		));
	}
	let request = &mut record.request;
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
	save(tx, grant, &requests).await?;
	Ok(answered)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
	key: String,
	request: HumanRequest,
}
pub(crate) async fn read_in(
	tx: &mut dyn TransactionExecutor,
	grant: Journal,
	lock: bool,
) -> Result<Vec<HumanRequest>> {
	let mut requests = records(tx, grant, lock).await?;
	let mut changed = false;
	for record in &mut requests {
		let request = &mut record.request;
		if crate::apps::execution::services::human_interaction::approval_expired(
			request,
			chrono::Utc::now(),
		) && request.response.as_ref()
			!= Some(&serde_json::json!({"approved":false,"expired":true}))
		{
			request.response = Some(serde_json::json!({"approved":false,"expired":true}));
			request.answered_by = Some("system".into());
			changed = true;
		}
	}
	if changed {
		if !lock {
			return read_locked(tx, grant).await;
		}
		save(tx, grant, &requests).await?;
	}
	Ok(requests.into_iter().map(|r| r.request).collect())
}
async fn read_locked(
	tx: &mut dyn TransactionExecutor,
	grant: Journal,
) -> Result<Vec<HumanRequest>> {
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
	create_in(
		tx,
		Journal::Scoped(grant),
		admission,
		workspace,
		kind,
		prompt,
		key,
	)
	.await
}
pub(crate) async fn read(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	lock: bool,
) -> Result<Vec<HumanRequest>> {
	read_in(tx, Journal::Scoped(grant), lock).await
}
pub(crate) async fn answer(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	admission: Uuid,
	id: Uuid,
	response: Value,
	actor: &str,
) -> Result<HumanRequest> {
	answer_in(tx, Journal::Scoped(grant), admission, id, response, actor).await
}
