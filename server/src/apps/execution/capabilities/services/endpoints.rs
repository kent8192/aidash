use super::{contracts::*, management::*, service, sessions};
use crate::{
	Error, Result,
	apps::execution::repositories::store::DutyTaskStart,
	authorization::{access::Access, execution, identity::Actor},
	domain::{NewTask, Run},
	federation::Federation,
	registry::EntityRef,
};
use reinhardt::query::{Alias, Expr, Order, PostgresQueryBuilder};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) async fn access(f: &Federation, actor: Actor) -> Result<Access> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Invalid(
			"core file operations require a tenant subject credential".into(),
		));
	};
	Access::begin(&f.store, &identity).await
}
fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
	Ok(serde_json::from_value(value)?)
}
pub(crate) async fn configure_agent(
	f: Federation,
	actor: Actor,
	id: String,
	input: super::configuration::Configure,
) -> Result<ConfiguredAgent> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Invalid(
			"core file operations require a tenant subject credential".into(),
		));
	};
	let mut access =
		Access::begin_change(&f.store, &identity, &["agent.configure", "registry.create"]).await?;
	let result = super::configuration::configure(&f.store, &mut access, id, input).await;
	typed(access.finish(result).await?)
}
async fn run_access(f: &Federation, actor: Actor, id: Uuid) -> Result<(Access, Run)> {
	let mut access = access(f, actor).await?;
	// Bind the caller to the immutable execution grant before session constraints
	// inspect its subject chain; the visibility checks still run afterward.
	// Authority is checked before existence is disclosed: a missing run and a run
	// outside the caller's immutable grant both answer with the same 403.
	let unavailable = |error| match error {
		Error::Forbidden | Error::NotFound(_) => Error::Forbidden,
		error => error,
	};
	let run = crate::apps::execution::models::Run::read_in(&mut **access.tx, id, None)
		.await?
		.ok_or(Error::Forbidden)?;
	execution::inherit_run_authority(&mut access, &run)
		.await
		.map_err(unavailable)?;
	let run = access.run_for_action(id).await?;
	let config = service::settings(&mut access, &run).await?;
	if config.core_capabilities.enabled() {
		sessions::initialize(&f.store, &mut access, &run, &config).await?;
	}
	Ok((access, run))
}

// Status and Stop belong to the viewer's Run authority. They do not execute
// tools or read the Run's Working Area under its original execution grant.
async fn viewer_run_access(f: &Federation, actor: Actor, id: Uuid) -> Result<(Access, Run)> {
	if matches!(&actor, Actor::DeploymentAdministrator) {
		return Err(Error::NotFound("run unavailable".into()));
	}
	let mut access = access(f, actor).await?;
	let run = access
		.run_for_interaction(id)
		.await
		.map_err(|error| match error {
			Error::Forbidden | Error::NotFound(_) => Error::NotFound("run unavailable".into()),
			error => error,
		})?;
	Ok((access, run))
}

pub(crate) async fn areas(
	f: Federation,
	actor: Actor,
	QueryInput(page): QueryInput<AreaQuery>,
) -> Result<AreaPage> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let rows: Vec<Area> = {
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = &access.identity.subject;
			let query_bind_3 = page.cursor.unwrap_or(Uuid::nil());
			let query_bind_5 = page.thread_id;
			crate::database::native::query_as(
				&sessions::select("core_areas")
					.and_where(
						Expr::col(Alias::new("tenant")).eq(Expr::value(query_bind_1.to_owned())),
					)
					.and_where(
						Expr::col(Alias::new("owner")).eq(Expr::value(query_bind_2.to_owned())),
					)
					.and_where(Expr::col(Alias::new("id")).gt(Expr::value(query_bind_3.to_owned())))
					.and_where(SimpleExpr::CustomWithExpr(
						"(?::uuid IS NULL OR thread_id = ?)".into(),
						vec![
							Expr::value(query_bind_5.to_owned()).into(),
							Expr::value(query_bind_5.to_owned()).into(),
						],
					))
					.order_by(Alias::new("id"), Order::Asc)
					.limit(51)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **access.tx)
			.await?
		};
		let next_cursor = if rows.len() == 51 {
			Some(rows[49].id)
		} else {
			None
		};
		let mut items = vec![];
		for area in rows.into_iter().take(50) {
			match sessions::authorize(&mut access, &area, "file.read").await {
				Ok(()) => items.push(area),
				Err(Error::Forbidden | Error::NotFound(_)) => {}
				Err(error) => return Err(error),
			}
		}
		Ok(AreaPage { items, next_cursor })
	}
	.await;
	access.finish(result).await
}
pub(crate) async fn area_for_run(f: Federation, actor: Actor, id: Uuid) -> Result<Area> {
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result = sessions::for_run(&mut access, &run).await;
	access.finish(result).await
}
pub(crate) async fn file_read(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: ToolFileRead,
) -> Result<Envelope> {
	dispatch(f, actor, id, "file_read", json!(input)).await
}
pub(crate) async fn file_search(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: ToolFileSearch,
) -> Result<Envelope> {
	dispatch(f, actor, id, "file_search", json!(input)).await
}
async fn dispatch(
	f: Federation,
	actor: Actor,
	id: Uuid,
	name: &str,
	input: Value,
) -> Result<Envelope> {
	use super::service::Invocation;
	use crate::apps::execution::repositories::subnodes;

	let (mut access, run) = run_access(&f, actor.clone(), id).await?;
	let key = Uuid::new_v4().to_string();
	let writes = !matches!(
		name,
		"file_read" | "file_search" | "skill_list" | "skill_load" | "skill_read"
	);
	let result = async {
		if writes {
			subnodes::require_writable(access.tx.native_mut(), run.thread_id).await?;
		}
		service::invoke(&f.store, &mut access, &run, name, input, &key).await
	}
	.await;
	let policy_revision = access.snapshot.revision;
	let request = match access.finish(result).await? {
		Invocation::Complete(output) => return Ok(*output),
		Invocation::Pod(request) => request,
	};
	let ran = request.execute(&run.metadata()).await;
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result = async {
		if access.snapshot.revision != policy_revision {
			return Err(Error::Forbidden);
		}
		if writes {
			subnodes::require_writable(access.tx.native_mut(), run.thread_id).await?;
		}
		let mut scope = crate::bootstrap::file_scope(Some(&f.store), &mut access, Some(&run));
		let output = ran?.settle(&mut scope, &run.metadata(), name, &key).await?;
		service::envelope(output)
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn shell(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: Shell,
) -> Result<OperationResult> {
	let value = dispatch(f, actor, id, "shell", json!(input)).await?;
	Ok(serde_json::from_value(json!(value))?)
}
pub(crate) async fn apply_patch(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: Patch,
) -> Result<Envelope> {
	dispatch(f, actor, id, "apply_patch", json!(input)).await
}
pub(crate) async fn shell_poll(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: OperationInput,
) -> Result<OperationResult> {
	let value = dispatch(f, actor, id, "shell_poll", json!(input)).await?;
	Ok(serde_json::from_value(json!(value))?)
}
pub(crate) async fn shell_cancel(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: OperationInput,
) -> Result<OperationResult> {
	let value = dispatch(f, actor, id, "shell_cancel", json!(input)).await?;
	Ok(serde_json::from_value(json!(value))?)
}
pub(crate) async fn session(f: Federation, actor: Actor, id: Uuid) -> Result<SessionStatus> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let area = sessions::load(&mut access, id).await?;
		sessions::status(&mut access, &area).await
	}
	.await;
	access.finish(result).await
}
/// The Run just admitted for `task` in this transaction.
async fn run_of_task(access: &mut Access, task: Uuid) -> Result<Run> {
	aidash_server::database::query_as(
		&sessions::select("task_executions")
			.and_where(Expr::col(Alias::new("task_id")).eq(Expr::value(task)))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **access.tx)
	.await
}

fn invalid_url_intent(error: aidash_domain::research::intent::InvalidUrlIntent) -> Error {
	Error::Invalid(format!("INVALID_URL_INTENT: {error}"))
}

/// Checks the pages one input asks the Run to read, before anything is
/// written. They are recorded beside the input, never in its text (ADR 0052).
fn check_url_intents(pages: &[String]) -> Result<()> {
	aidash_domain::research::intent::validate(pages).map_err(invalid_url_intent)
}

/// Records the pages `requested_by` asked `run` to read with one input:
/// its Task when `input_key` is `None`. A Run whose Agent binds no Page
/// reading operation cannot act on them, so they are refused instead.
async fn record_url_intents(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	run: &Run,
	input_key: Option<&str>,
	pages: &[String],
	requested_by: &str,
) -> Result<()> {
	if pages.is_empty() {
		return Ok(());
	}
	let reads = run
		.context
		.binding_snapshot
		.as_deref()
		.is_some_and(aidash_domain::research::intent::reads_pages);
	if !reads {
		return Err(invalid_url_intent(
			aidash_domain::research::intent::InvalidUrlIntent::NoPageReading,
		));
	}
	crate::apps::execution::models::url_intents::UrlIntentRecord::record(
		tx,
		run.id,
		input_key,
		pages,
		requested_by,
	)
	.await
}

pub(crate) async fn steer(f: Federation, actor: Actor, id: Uuid, input: Steer) -> Result<Steered> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let area = sessions::load(&mut access, id).await?;
		let digest = crate::registry::digest(&json!(["steer", id, input]));
		if let Some(result) = sessions::cached(&mut access, input.idempotency_key, &digest).await? {
			return Ok((Some(serde_json::from_value::<Steered>(result)?), None));
		}
		if sessions::status(&mut access, &area).await?.active_run_id != Some(input.expected_run_id)
		{
			return Err(Error::Conflict(
				"ACTIVE_RUN_CHANGED: reload before steering".into(),
			));
		}
		let run = access.run_for_interaction(input.expected_run_id).await?;
		access
			.require(&access.resource("run", run.id, json!({})), "run.message")
			.await?;
		access
			.require(
				&access.resource("thread", area.thread_id, json!({})),
				"message.create",
			)
			.await?;
		let limit = f.run_message_limit(&run).await?;
		let sender = access.identity.subject.clone();
		check_url_intents(&input.read_pages)?;
		Ok((None, Some((run, sender, limit, digest))))
	}
	.await;
	let (cached, ready) = match result {
		Ok(result) => result,
		Err(error) => return access.finish(Err(error)).await,
	};
	if let Some(cached) = cached {
		return access.finish(Ok(cached)).await;
	}
	let (run, sender, limit, digest) = ready.expect("fresh steer input");
	let tenant = access.identity.tenant.clone();
	let mut access = access.into_native()?;
	let result = async {
		f.store
			.accept_run_message_in(
				access.tx.as_mut(),
				run.id,
				&sender,
				&input.content,
				&format!("steer:{}:{}", run.id, input.idempotency_key),
				limit,
			)
			.await?;
		record_url_intents(
			access.tx.as_mut(),
			&run,
			Some(&format!("steer:{}:{}", run.id, input.idempotency_key)),
			&input.read_pages,
			&sender,
		)
		.await?;
		let result = Steered {
			accepted_run_id: run.id,
		};
		crate::apps::execution::capabilities::models::CoreRequest::remember(
			access.tx.as_mut(),
			&tenant,
			&sender,
			input.idempotency_key,
			&digest,
			json!(result),
		)
		.await?;
		Ok(result)
	}
	.await;
	let result = access.finish(result).await?;
	let run = f
		.store
		.run(
			&aidash_application::tenancy::TenancyScope::tenant(&tenant)?,
			result.accepted_run_id,
		)
		.await?;
	if let Err(error) = f.deliver_run_messages(&run).await {
		tracing::warn!(run_id=%run.id, %error, "steer awaits durable delivery");
	}
	f.notify.notify_waiters();
	Ok(result)
}

pub(crate) async fn skill_import(
	f: Federation,
	actor: Actor,
	input: crate::skill_import::ImportRequest,
) -> Result<super::skills::SkillAttachment> {
	let mut access = access(&f, actor).await?;
	access
		.require(
			&access.resource("skill_attachment", "import", json!({})),
			"skill.import",
		)
		.await?;
	access.finish(Ok(())).await?;
	let result = crate::skill_import::import(input).await?;
	super::skills::imported(
		result
			.selected
			.ok_or_else(|| Error::Invalid(format!("SELECT_SKILL: {}", json!(result.skills))))?,
	)
}
pub(crate) async fn skill_list(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::skills::SkillList,
) -> Result<Envelope> {
	dispatch(f, actor, id, "skill_list", json!(input)).await
}
pub(crate) async fn skill_load(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::skills::SkillLoad,
) -> Result<Envelope> {
	dispatch(f, actor, id, "skill_load", json!(input)).await
}
pub(crate) async fn skill_read(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::skills::SkillRead,
) -> Result<Envelope> {
	dispatch(f, actor, id, "skill_read", json!(input)).await
}

/// Checks that `thread` is a live Thread the caller may read, holding
/// its row lock like Thread deletion does, so a deletion that already
/// passed its final area page cannot race the caller's write.
pub(crate) async fn readable_thread(access: &mut Access, thread: Uuid) -> Result<()> {
	crate::apps::workspaces::models::Thread::lock_existing(&mut *access.tx, thread)
		.await
		.map_err(|error| match error {
			Error::NotFound(_) => Error::NotFound("thread unavailable".into()),
			error => error,
		})?;
	super::thread_lifecycle::visible(&mut access.tx, thread).await?;
	let resource = access.thread(thread).await?;
	access.require(&resource, "thread.read").await?;
	Ok(())
}

/// Starts an Agent's Run in a Thread inside `access`'s transaction, as its
/// subject: a Root Task and its delegation. Duties start their Tasks through
/// it as their Requester (#557, #1011). The Task records its Duty before the
/// claim pins the Duty's mission and procedure. The Task ID is None when the
/// idempotency key returned an earlier start.
pub(crate) async fn start_in_thread(
	f: &Federation,
	access: &mut Access,
	(thread, agent): (Uuid, String),
	input: Enqueue,
	duty: Option<DutyTaskStart>,
) -> Result<(Area, Option<Uuid>)> {
	// Thread deletion takes this same row lock before writing its
	// tombstone, so a Run cannot commit behind a delete that already passed
	// its final area page.
	readable_thread(access, thread).await?;
	let digest = crate::registry::digest(&json!(["thread_run", thread, agent, input]));
	if let Some(previous) = sessions::cached(access, input.idempotency_key, &digest).await? {
		let id = serde_json::from_value(previous["area_id"].clone())?;
		return Ok((sessions::load(access, id).await?, None));
	}
	check_url_intents(&input.read_pages)?;
	let resource = access.thread(thread).await?;
	access.require(&resource, "task.create").await?;
	let creator = access.identity.subject.clone();
	let task = f
		.store
		.create_task_with_duty_in(
			&mut access.tx,
			thread,
			&NewTask {
				title: input.title,
				description: input.description,
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			&creator,
			None,
			duty,
		)
		.await?;
	let delegation = execution::delegate_in(
		f,
		access,
		task.id,
		&EntityRef {
			id: agent,
			version: input.agent_version,
		},
	)
	.await?;
	let run = run_of_task(access, task.id).await?;
	let _ = delegation;
	record_url_intents(&mut *access.tx, &run, None, &input.read_pages, &creator).await?;
	let area = sessions::for_run(access, &run).await?;
	sessions::cache(
		access,
		input.idempotency_key,
		&digest,
		&json!({"area_id":area.id}),
	)
	.await?;
	Ok((area, Some(task.id)))
}

pub(crate) async fn thread_run(
	f: Federation,
	actor: Actor,
	target: (Uuid, String),
	input: Enqueue,
) -> Result<Area> {
	let mut access = access(&f, actor).await?;
	let result = start_in_thread(&f, &mut access, target, input, None)
		.await
		.map(|(area, _)| area);
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(result)
}

pub(crate) async fn outbound_get(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::approvals::Outbound,
) -> Result<Envelope> {
	dispatch(f, actor, id, "outbound_get", json!(input)).await
}

pub(crate) async fn outbound_history(
	f: Federation,
	actor: Actor,
	id: Uuid,
	QueryInput(page): QueryInput<Page>,
) -> Result<OutboundPage> {
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result = async {
		sessions::context_authority(&mut access, &run).await?;
		let records: Vec<super::records::Record> = {
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = &access.identity.subject;
			let query_bind_3 = id.to_string();
			let query_bind_4 = page.cursor.unwrap_or(Uuid::nil());
			crate::database::native::query_as(
				&sessions::select("core_records")
					.and_where(
						Expr::col(Alias::new("tenant")).eq(Expr::value(query_bind_1.to_owned())),
					)
					.and_where(
						Expr::col(Alias::new("owner")).eq(Expr::value(query_bind_2.to_owned())),
					)
					.and_where(
						Expr::col(Alias::new("kind")).eq(reinhardt::query::Expr::value("outbound")),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(data->>'run_id' = ?)".into(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					))
					.and_where(Expr::col(Alias::new("id")).gt(Expr::value(query_bind_4.to_owned())))
					.order_by(Alias::new("id"), Order::Asc)
					.limit(51)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **access.tx)
			.await?
		};
		let next_cursor = (records.len() == 51).then(|| records[49].id);
		let mut items = vec![];
		for record in records.into_iter().take(50) {
			let mut item = super::approvals::view(&f.store, &mut access, &run, &record).await?;
			item["url"] = record.data["url"].clone();
			item["final_url"] = record.data["final_url"].clone();
			items.push(serde_json::from_value(item)?);
		}
		Ok(OutboundPage { items, next_cursor })
	}
	.await;
	access.finish(result).await
}
/// Approval records that end with their Run, so a pending one whose Run has
/// ended is closed rather than left to be decided.
fn ends_with_run(row: &super::records::Record) -> bool {
	matches!(row.kind.as_str(), "tool_call" | "sampling") && row.state == "pending"
}
/// The most cards one page of approvals holds.
const APPROVAL_PAGE: usize = 50;
/// The most rows one request reads before it returns a short page and a cursor.
/// Who may decide an approval is judged per row (ADR 0135), so rows the reader
/// cannot see are skipped after the query; this bounds the work a page of
/// mostly invisible rows costs.
const APPROVAL_SCAN: usize = 10 * APPROVAL_PAGE;
pub(crate) async fn approval_list(
	f: Federation,
	actor: Actor,
	QueryInput(query): QueryInput<ApprovalQuery>,
) -> Result<ApprovalPage> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let subject = access.identity.subject.clone();
		// Only a requester or a `capability_approver` can be an Eligible
		// approver of anything (ADR 0135), so anyone else is shown only their
		// own requests. Who may see each row is judged below, as policy stands.
		let may_approve_others =
			access
				.snapshot
				.bundle
				.subjects
				.get(&subject)
				.is_some_and(|person| {
					person.enabled
						&& person.kind == aidash_domain::policy::SubjectKind::User
						&& person.attributes["capability_approver"] == true
				});
		let (kinds, state): (&[&str], _) = match query.state {
			Some(ApprovalState::Pending) => {
				(&["outbound", "tool_call", "sampling"], Some("pending"))
			}
			Some(ApprovalState::Active) => (&["grant"], Some("active")),
			None => (&["outbound", "grant", "tool_call", "sampling"], None),
		};
		let mut cursor = query.cursor.unwrap_or(Uuid::nil());
		let mut scanned = 0;
		let mut next_cursor = None;
		let mut items = vec![];
		'pages: loop {
			let mut select = sessions::select("core_records")
				.and_where(
					Expr::col(Alias::new("tenant"))
						.eq(Expr::value(access.identity.tenant.to_owned())),
				)
				.and_where(Expr::col(Alias::new("id")).gt(Expr::value(cursor)))
				.and_where(Expr::col(Alias::new("kind")).is_in(kinds.iter().copied()))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(APPROVAL_PAGE as u64 + 1)
				.to_owned();
			if !may_approve_others {
				select.and_where(Expr::col(Alias::new("owner")).eq(Expr::value(subject.clone())));
			}
			if let Some(state) = state {
				select
					.and_where(Expr::col(Alias::new("state")).eq(Expr::value(state)))
					.and_where(
						Expr::col(Alias::new("expires_at")).gt(Expr::value(chrono::Utc::now())),
					);
			}
			if query.state == Some(ApprovalState::Pending) {
				// A tool call approval ends with its Run: an inbox never
				// offers one whose Run has ended, since no decision on it could take
				// effect.
				select.and_where(Expr::cust(
					"(kind NOT IN ('tool_call', 'sampling') OR NOT EXISTS (SELECT 1 FROM task_executions r \
					 WHERE r.id::text = core_records.data->>'run_id' \
					 AND (r.phase IN ('COMPLETED', 'FAILED', 'CANCELLED') OR r.control = 'CANCELLED')))",
				));
			}
			let mut rows: Vec<super::records::Record> =
				crate::database::native::query_as(&select.to_string(PostgresQueryBuilder))
					.fetch_all(&mut **access.tx)
					.await?;
			let more = rows.len() > APPROVAL_PAGE;
			rows.truncate(APPROVAL_PAGE);
			// Every view, not only the inbox, shows a pending tool call
			// approval whose Run has ended as closed, as a decision on it would close
			// it, so no reader is offered it.
			let waiting: Vec<Uuid> = rows
				.iter()
				.filter(|row| ends_with_run(row))
				.filter_map(|row| serde_json::from_value(row.data["run_id"].clone()).ok())
				.collect();
			let ended: std::collections::HashSet<Uuid> = if waiting.is_empty() {
				Default::default()
			} else {
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("task_executions"))
						.and_where(
							Expr::col(Alias::new("id")).is_in(waiting.into_iter().map(Expr::value)),
						)
						.and_where(
							Expr::col(Alias::new("phase"))
								.is_in(["COMPLETED", "FAILED", "CANCELLED"])
								.or(Expr::col(Alias::new("control")).eq(Expr::value("CANCELLED"))),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_all(&mut **access.tx)
				.await?
				.into_iter()
				.collect()
			};
			let count = rows.len();
			for (index, row) in rows.into_iter().enumerate() {
				cursor = row.id;
				scanned += 1;
				let reader = super::approvals::reader(&mut access, &row).await?;
				// An inbox lists only what the reader may decide, or revoke as an
				// approver; otherwise their own requests are listed too.
				let listed = match query.role {
					Some(ApprovalRole::Approver) => reader.approver,
					None => reader.visible,
				};
				if listed {
					items.push(
						approval_card(&f, &mut access, &subject, &row, reader, &ended).await?,
					);
				}
				let rest = more || index + 1 < count;
				if items.len() == APPROVAL_PAGE || (scanned >= APPROVAL_SCAN && rest) {
					next_cursor = rest.then_some(cursor);
					break 'pages;
				}
			}
			if !more {
				break;
			}
		}
		Ok(json!({"items": items, "next_cursor": next_cursor}))
	}
	.await;
	typed(access.finish(result).await?)
}
/// One approval card as `subject` may see it.
async fn approval_card(
	f: &Federation,
	access: &mut Access,
	subject: &str,
	row: &super::records::Record,
	reader: aidash_application::capabilities::approvals::Reader,
	ended: &std::collections::HashSet<Uuid>,
) -> Result<Value> {
	let run: Uuid = serde_json::from_value(row.data["run_id"].clone())?;
	let run_ended = ends_with_run(row) && ended.contains(&run);
	// A Run grant past its `expires_at` no longer admits anything, so
	// it reads as expired rather than active and revocable.
	let expired = matches!(row.state.as_str(), "pending" | "active")
		&& row.expires_at.is_some_and(|t| t < chrono::Utc::now());
	let data = &row.data;
	let state = if run_ended {
		"closed"
	} else if expired {
		"expired"
	} else {
		&row.state
	};
	// A rule can name an outbound read or a tool call, and
	// only the person the Run's delegation chain starts at may make one. Only
	// someone who may decide it can, which also skips a record whose Thread
	// is no longer live, as its Run's context no longer loads.
	let allow_always = state == "pending"
		&& reader.approver
		&& matches!(row.kind.as_str(), "outbound" | "tool_call" | "sampling")
		&& data["subjects"][0] == json!(subject)
		&& super::approvals::can_allow_always(&f.store, access, row).await?;
	// The Thread offers a pending card inline only when the reader decides it
	// as the requester at the root of the Run's delegation chain (#896).
	let decide_inline = state == "pending"
		&& matches!(row.kind.as_str(), "outbound" | "tool_call")
		&& reader.approver
		&& row.owner == subject
		&& data["subjects"][0] == json!(subject);
	// Eligible approvers see only the scoped card, never Run output.
	Ok(json!({
		"id": row.id,
		"kind": row.kind,
		"run_id": run,
		"area_id": row.area_id,
		"state": state,
		"revision": row.revision,
		"requester": row.owner,
		"targets": data["targets"],
		"action": data["action"].as_str().or_else(|| data["actions"][0].as_str()).unwrap_or("network.get"),
		"expires_at": row.expires_at,
		"grant_id": data["grant_id"],
		"viewer_is_approver": reader.approver,
		"viewer_can_decide_inline": decide_inline,
		"agent_id": data["agent_id"],
		"agent_version": data["agent_version"],
		"tool": data["tool"],
		"version": if data["tool"].is_string() { data["version"].clone() } else { Value::Null },
		"plugin_tool": data["plugin_tool"],
		"operation": data["operation"],
		"call": if row.kind == "tool_call" { data["call"].clone() } else { Value::Null },
		"sampling_target": data["sampling_target"],
		"sampling": data["sampling"],
		// A Run grant names who gave it; an approval, who decided it.
		"decided_by": if row.kind == "grant" { data["approver"].clone() } else { data["decided_by"].clone() },
		"decided_at": data["decided_at"],
		"given_by": data["given_by"],
		"refused_by": data["refused_by"],
		"allow_always": allow_always,
		"closed_reason": if run_ended { json!("run_ended") } else { data["closed_reason"].clone() },
	}))
}
pub(crate) async fn approval_decide(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::approvals::ApprovalDecision,
) -> Result<ApprovalDecided> {
	let mut access = access(&f, actor).await?;
	let result = super::approvals::decide(&f.store, &mut access, id, input).await;
	let value = access.finish(result).await?;
	f.notify.notify_waiters();
	typed(value)
}
pub(crate) async fn approval_revoke(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::approvals::Revoke,
) -> Result<Revoked> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let kind = super::approvals::revocable_kind(&mut access, id).await?;
		super::approvals::revoke(&mut access, id, kind, input).await
	}
	.await;
	typed(access.finish(result).await?)
}
pub(crate) async fn grant_revoke(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::approvals::Revoke,
) -> Result<Revoked> {
	let mut access = access(&f, actor).await?;
	let result = super::approvals::revoke(&mut access, id, "grant", input).await;
	typed(access.finish(result).await?)
}

pub(crate) async fn python(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::python::Python,
) -> Result<Envelope> {
	dispatch(f, actor, id, "code_interpreter", json!(input)).await
}
pub(crate) async fn python_poll(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: OperationInput,
) -> Result<Envelope> {
	dispatch(f, actor, id, "python_poll", json!(input)).await
}
pub(crate) async fn python_cancel(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: OperationInput,
) -> Result<Envelope> {
	dispatch(f, actor, id, "python_cancel", json!(input)).await
}

pub(crate) async fn reference_upload(
	f: Federation,
	actor: Actor,
	input: super::references::Upload,
) -> Result<super::references::Reference> {
	let mut access = access(&f, actor).await?;
	let result = super::references::start(&f.store, &mut access, input).await;
	access.finish(result).await
}
pub(crate) async fn reference_batch(
	f: Federation,
	actor: Actor,
	input: super::references::Batch,
) -> Result<super::references::BatchResult> {
	let mut access = access(&f, actor).await?;
	let result = super::references::batch(&f.store, &mut access, input).await;
	access.finish(result).await
}
pub(crate) async fn reference_chunk(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::references::Chunk,
) -> Result<super::references::Reference> {
	let mut access = access(&f, actor).await?;
	let result = super::references::chunk(&f.store, &mut access, id, input).await;
	access.finish(result).await
}
pub(crate) async fn reference_commit(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<super::references::Reference> {
	let mut access = access(&f, actor).await?;
	let result = super::references::commit(&f.store, &mut access, id).await;
	access.finish(result).await
}
pub(crate) async fn reference_inspect(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<super::references::Reference> {
	let mut access = access(&f, actor).await?;
	let result = super::references::inspect(&mut access, id).await;
	access.finish(result).await
}

pub(crate) async fn reference_revocation(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<ReferenceRevocation> {
	let mut access = access(&f, actor).await?;
	let result = super::references::revocation(&mut access, id).await;
	access.finish(result).await
}

pub(crate) async fn reference_revoke(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: ExpectedRevision,
) -> Result<ReferenceRevoked> {
	let mut access = access(&f, actor).await?;
	let result = super::references::revoke(&mut access, id, input.expected_revision).await;
	access.finish(result).await?;
	typed(json!({"reference_id":id,"status":"revoked"}))
}

async fn download(
	store: &crate::store::Store,
	access: &mut Access,
	file: FileEntry,
	offset: u64,
) -> Result<DownloadChunk> {
	use base64::Engine;
	let bytes = store.capabilities.read_chunk(access, &file, offset).await?;
	let end = offset + bytes.len() as u64;
	Ok(DownloadChunk {
		next_offset: (end < file.size).then_some(end),
		file,
		offset,
		data: base64::engine::general_purpose::STANDARD.encode(bytes),
	})
}
pub(crate) async fn reference_download(
	f: Federation,
	actor: Actor,
	id: Uuid,
	QueryInput(input): QueryInput<Offset>,
) -> Result<DownloadChunk> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let reference = super::references::get(&mut access, id, "reference.read").await?;
		let file = serde_json::from_value(reference.data["original"].clone())
			.map_err(|_| Error::Conflict("REFERENCE_NOT_READY".into()))?;
		download(&f.store, &mut access, file, input.offset.unwrap_or(0)).await
	}
	.await;
	access.finish(result).await
}
pub(crate) async fn file_download(
	f: Federation,
	actor: Actor,
	(id, file): (Uuid, Uuid),
	QueryInput(input): QueryInput<Offset>,
) -> Result<DownloadChunk> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let area = sessions::load(&mut access, id).await?;
		let entry = if let Some(entry) = service::files(&area)?
			.into_iter()
			.find(|entry| entry.file_id == file)
		{
			service::available(&area)?;
			// A Run can read a Reference file; downloading it, or a copy of it,
			// stays with its uploader (ADR 0114).
			if entry.provenance["kind"] == "reference" {
				let id = serde_json::from_value(entry.provenance["id"].clone())
					.map_err(|_| Error::Forbidden)?;
				super::references::get(&mut access, id, "reference.read").await?;
			}
			entry
		} else {
			let meta: Option<(String, i64, String)> = {
				let query_bind_1 = file;
				let query_bind_2 = id;
				let query_bind_3 = &access.identity.tenant;
				crate::database::native::query_as(
					&reinhardt::query::Query::select()
						.columns(["digest", "size", "kind"].map(Alias::new))
						.from(Alias::new("core_objects"))
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("area_id"))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("tenant"))
								.eq(Expr::value(query_bind_3.to_owned())),
						)
						.and_where(Expr::col(Alias::new("kind")).is_in([
							"display",
							"output",
							"network_output",
						]))
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["digest", "size", "kind"])
				.fetch_optional(&mut **access.tx)
				.await?
			};
			if let Some((digest, size, kind)) = meta {
				service::available(&area)?;
				FileEntry {
					file_id: file,
					path: file.to_string(),
					digest,
					size: size as u64,
					media_type: if kind == "display" {
						"image/png"
					} else if kind == "network_output" {
						"application/octet-stream"
					} else {
						"text/plain; charset=utf-8"
					}
					.into(),
					scope: FileScope::Working,
					provenance: json!({"kind":kind,"area_id":id}),
				}
			} else {
				service::available(&area)?;
				return Err(Error::NotFound("file unavailable".into()));
			}
		};
		download(&f.store, &mut access, entry, input.offset.unwrap_or(0)).await
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn operation_history(
	f: Federation,
	actor: Actor,
	id: Uuid,
	QueryInput(page): QueryInput<Page>,
) -> Result<OperationPage> {
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result=async {
        let area=sessions::for_run(&mut access,&run).await?;
        let rows:Vec<(Uuid,String,String)>={ let query_bind_1 = run.id; let query_bind_2 = area.id; let query_bind_3 = page.cursor.unwrap_or(Uuid::nil()); crate::database::native::query_as(&reinhardt::query::Query::select().columns(["id","kind","state"].map(Alias::new)).from(Alias::new("core_operations"))
            .and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(query_bind_1.to_owned())))
            .and_where(Expr::col(Alias::new("area_id")).eq(Expr::value(query_bind_2.to_owned())))
            .and_where(Expr::col(Alias::new("id")).gt(Expr::value(query_bind_3.to_owned()))).order_by(Alias::new("id"),Order::Asc).limit(33).to_string(PostgresQueryBuilder)).columns(&["id", "kind", "state"]).fetch_all(&mut **access.tx).await? };
        let cursor=if rows.len()==33 {Some(rows[31].0)}else{None};
        Ok(json!({"items":rows.into_iter().take(32).map(|(id,kind,status)|json!({"operation_id":id,"kind":kind,"status":status})).collect::<Vec<_>>(),"next_cursor":cursor}))
    }.await;
	typed(access.finish(result).await?)
}

pub(crate) async fn reference_list(
	f: Federation,
	actor: Actor,
	QueryInput(page): QueryInput<Page>,
) -> Result<ReferencePage> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let ids: Vec<Uuid> = {
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = &access.identity.subject;
			let query_bind_3 = page.cursor.unwrap_or(Uuid::nil());
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_records"))
					.and_where(
						Expr::col(Alias::new("kind"))
							.eq(reinhardt::query::Expr::value("reference")),
					)
					.and_where(
						Expr::col(Alias::new("tenant")).eq(Expr::value(query_bind_1.to_owned())),
					)
					.and_where(
						Expr::col(Alias::new("owner")).eq(Expr::value(query_bind_2.to_owned())),
					)
					.and_where(Expr::col(Alias::new("state")).is_not_in(["revoked", "expired"]))
					.and_where(Expr::col(Alias::new("id")).gt(Expr::value(query_bind_3.to_owned())))
					.order_by(Alias::new("id"), Order::Asc)
					.limit(51)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_all(&mut **access.tx)
			.await?
		};
		let next_cursor = if ids.len() == 51 { Some(ids[49]) } else { None };
		let mut items = vec![];
		for id in ids.into_iter().take(50) {
			match super::references::inspect(&mut access, id).await {
				Ok(v) => items.push(v),
				Err(Error::Forbidden | Error::NotFound(_)) => {}
				Err(e) => return Err(e),
			}
		}
		Ok(ReferencePage { items, next_cursor })
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn thread_delete(
	f: Federation,
	actor: Actor,
	thread: Uuid,
	input: super::thread_lifecycle::DeleteThread,
) -> Result<DeletedThread> {
	let mut access = access(&f, actor).await?;
	let result = super::thread_lifecycle::delete(&f.store, &mut access, thread, input).await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	typed(result)
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use reinhardt::Query as QueryInput;

#[derive(Clone)]
pub struct CapabilitiesManagement {
	pub(crate) runtime: Federation,
}
// Preserve a statement before the value until reinhardt-web#6441 is fixed.
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> CapabilitiesManagement {
	tracing::trace!(
		service = "CapabilitiesManagement",
		"creating injectable service"
	);
	CapabilitiesManagement { runtime }
}

use reinhardt::injectable;

pub(crate) use crate::apps::execution::capabilities::serializers::endpoints::{
	ApprovalQuery, ApprovalRole, ApprovalState, AreaQuery, DownloadChunk, ExpectedRevision, Offset,
	OutboundPage, Page, ReferencePage,
};

use reinhardt::query::SimpleExpr;

/// Whether this Deployment offers Previews and where.
pub(crate) fn preview_availability(f: &Federation) -> aidash_application::preview::Availability {
	aidash_application::preview::Availability::of(
		f.config
			.preview
			.as_ref()
			.map(|preview| preview.domain.as_str()),
	)
}

/// Authorize the File version as a download, then issue its one-time ticket.
pub(crate) async fn preview_ticket(
	f: Federation,
	actor: Actor,
	input: crate::apps::execution::capabilities::serializers::previews::PreviewTicketInput,
	origin: String,
) -> Result<aidash_application::preview::Issued> {
	let mut access = access(&f, actor).await?;
	let preview = f.config.preview.as_ref();
	let result = {
		let mut scope = crate::apps::execution::repositories::previews::Scope {
			access: &mut access,
			store: &f.store,
			domain: preview.map(|preview| preview.domain.as_str()),
			public_port: preview.and_then(|preview| preview.public_port),
			origin: &origin,
			peer: None,
		};
		aidash_application::preview::issue(&mut scope, input.area_id, input.file_id).await
	};
	access.finish(result.map_err(Into::into)).await
}

/// The Run's Live preview, or `None` when it has never started one.
pub(crate) async fn live_preview(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<crate::apps::execution::capabilities::serializers::previews::LivePreviewStatus> {
	use crate::apps::execution::capabilities::serializers::previews::LivePreviewStatus;
	let (mut access, run) = viewer_run_access(&f, actor.clone(), id).await?;
	let domain = f.config.preview.as_ref().map(|p| p.domain.clone());
	let result = crate::apps::execution::repositories::live_previews::status(
		&f.store,
		&mut access,
		&run,
		domain,
	)
	.await;
	let status = access.finish(result).await?;
	let may_stop = may_stop(&f, &actor, &run, id).await?;
	Ok(LivePreviewStatus::of(status, may_stop))
}

/// The viewer may stop a Live preview when they requested the Run or may
/// cancel it; viewing alone does not allow it (ADR 0070).
async fn may_stop(
	f: &Federation,
	actor: &Actor,
	run: &crate::domain::Run,
	id: Uuid,
) -> Result<bool> {
	let identity = match actor {
		Actor::DeploymentAdministrator => return Ok(false),
		Actor::Subject(identity) => identity.clone(),
	};
	let requester = aidash_application::authorization::execution::grant(
		&crate::bootstrap::execution_grants(&f.store),
		&run.metadata(),
	)
	.await?
	.is_some_and(|grant| grant.root_subject == identity.subject);
	if requester {
		return Ok(true);
	}
	match aidash_application::authorization::runs::authorize_run(
		&crate::bootstrap::run_control_repository(f, &identity),
		id,
	)
	.await
	{
		Ok(_) => Ok(true),
		Err(aidash_application::Error::Forbidden | aidash_application::Error::NotFound(_)) => {
			Ok(false)
		}
		Err(error) => Err(error.into()),
	}
}

/// Stop the Run's Live preview. Its requester and whoever may cancel the Run
/// may stop it; viewing alone does not allow it (ADR 0070).
pub(crate) async fn live_preview_stop(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<crate::apps::execution::capabilities::serializers::previews::LivePreviewStatus> {
	use crate::apps::execution::capabilities::serializers::previews::LivePreviewStatus;
	use crate::apps::execution::repositories::live_previews as previews;
	use aidash_domain::preview::live::StopReason;
	let domain = f.config.preview.as_ref().map(|p| p.domain.clone());
	let (mut access, run) = viewer_run_access(&f, actor.clone(), id).await?;
	let identity = access.identity.clone();
	let requester = aidash_application::authorization::execution::grant(
		&crate::bootstrap::execution_grants(&f.store),
		&run.metadata(),
	)
	.await?
	.is_some_and(|grant| grant.root_subject == identity.subject);
	if !requester {
		aidash_application::authorization::runs::authorize_run(
			&crate::bootstrap::run_control_repository(&f, &identity),
			id,
		)
		.await?;
	}
	let status = previews::status(&f.store, &mut access, &run, domain.clone()).await;
	let status = access.finish(status).await?;
	let Some(status) = status else {
		return Ok(LivePreviewStatus::of(None, false));
	};
	if status.state.is_stopped() {
		return Ok(LivePreviewStatus::of(Some(status), false));
	}
	let stopped = {
		let (mut access, run) = viewer_run_access(&f, actor.clone(), id).await?;
		let result = previews::request_stop(
			&f.store,
			&mut access,
			&run,
			domain.clone(),
			status.id,
			StopReason::Stopped,
		)
		.await;
		access.finish(result).await?
	};
	if previews::terminate(&f.store, &stopped)
		.await
		.unwrap_or(false)
	{
		let (mut access, run) = viewer_run_access(&f, actor, id).await?;
		let result = previews::settle(&f.store, &mut access, &run, domain, stopped.id).await;
		access.finish(result).await?;
	}
	let (mut access, run) = viewer_run_access(&f, Actor::Subject(identity), id).await?;
	let result = previews::status(&f.store, &mut access, &run, None).await;
	access
		.finish(result)
		.await
		.map(|status| LivePreviewStatus::of(status, false))
}

/// Issue a one-time ticket for a Run's Live preview under the same authority as
/// downloading that Working Area's files.
pub(crate) async fn preview_live_ticket(
	f: Federation,
	actor: Actor,
	input: crate::apps::execution::capabilities::serializers::previews::LivePreviewTicketInput,
	origin: String,
) -> Result<aidash_application::preview::Issued> {
	let mut access = access(&f, actor).await?;
	let preview = f.config.preview.as_ref();
	let result = {
		let mut scope = crate::apps::execution::repositories::previews::Scope {
			access: &mut access,
			store: &f.store,
			domain: preview.map(|preview| preview.domain.as_str()),
			public_port: preview.and_then(|preview| preview.public_port),
			origin: &origin,
			peer: None,
		};
		aidash_application::preview::issue_live(&mut scope, input.live_preview_id).await
	};
	access.finish(result.map_err(Into::into)).await
}

/// A Tenant's stored Live preview limits and the Deployment's ceilings.
pub(crate) async fn preview_limits(
	f: Federation,
	actor: Actor,
	tenant: String,
) -> Result<crate::apps::execution::capabilities::serializers::previews::TenantPreviewLimits> {
	use crate::apps::execution::capabilities::{
		models::CorePreviewLimits, serializers::previews::TenantPreviewLimits,
	};
	use aidash_domain::identity::tenant_administration::{COLLECTION, TEST_LIMITS_UPDATE};
	use reinhardt::db::orm::Model;
	let actions = crate::apps::registry::workbench::repositories::authority::TenantActions::new(
		actor,
		f.config.deployment.to_string(),
	);
	let mut tx = f
		.store
		.pool
		.begin(&aidash_application::tenancy::TenancyScope::tenant(&tenant)?)
		.await?;
	let policy = f.store.pool.dashboard_policy();
	actions
		.require(
			&mut *tx,
			policy.as_ref(),
			&tenant,
			TEST_LIMITS_UPDATE,
			COLLECTION,
		)
		.await?;
	let stored = CorePreviewLimits::objects()
		.filter(CorePreviewLimits::field_tenant().eq(&tenant))
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop();
	actions.record_denial(&f.store.pool).await?;
	tx.commit().await?;
	let ceilings = f.store.preview_limits.clone();
	let (idle_seconds, absolute_seconds, revision) = stored
		.map(|row| (row.idle_seconds, row.absolute_seconds, row.revision))
		.unwrap_or((ceilings.idle_seconds, ceilings.absolute_seconds, 0));
	Ok(TenantPreviewLimits {
		tenant,
		idle_seconds,
		absolute_seconds,
		revision,
		ceiling_idle_seconds: ceilings.idle_seconds,
		ceiling_absolute_seconds: ceilings.absolute_seconds,
	})
}

/// Lower a Tenant's Live preview limits within the Deployment's ceilings.
pub(crate) async fn set_preview_limits(
	f: Federation,
	actor: Actor,
	input: crate::apps::execution::capabilities::serializers::previews::PreviewLimitInput,
) -> Result<crate::apps::execution::capabilities::serializers::previews::TenantPreviewLimits> {
	use crate::apps::execution::capabilities::{
		models::CorePreviewLimits, serializers::previews::TenantPreviewLimits,
	};
	use aidash_domain::identity::tenant_administration::{COLLECTION, TEST_LIMITS_UPDATE};
	use aidash_domain::preview::live::Limits;
	use reinhardt::db::orm::Model;
	if input.tenant.is_empty() {
		return Err(Error::Invalid("tenant is required".into()));
	}
	let requested = Limits::new(input.idle_seconds, input.absolute_seconds)?;
	let ceilings = f.store.preview_limits.clone();
	let limits = requested.lower_than(&ceilings)?;
	let actions = crate::apps::registry::workbench::repositories::authority::TenantActions::new(
		actor,
		f.config.deployment.to_string(),
	);
	let mut tx = f
		.store
		.pool
		.begin(&aidash_application::tenancy::TenancyScope::tenant(
			&input.tenant,
		)?)
		.await?;
	let policy = f.store.pool.dashboard_policy();
	actions
		.require(
			&mut *tx,
			policy.as_ref(),
			&input.tenant,
			TEST_LIMITS_UPDATE,
			COLLECTION,
		)
		.await?;
	let stored = CorePreviewLimits::objects()
		.filter(CorePreviewLimits::field_tenant().eq(&input.tenant))
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop();
	if stored
		.as_ref()
		.is_some_and(|row| row.revision != input.expected_revision)
	{
		return Err(Error::Conflict("PREVIEW_LIMITS_REVISION_CHANGED".into()));
	}
	let revision = stored.as_ref().map_or(1, |row| row.revision + 1);
	if stored.is_some() {
		let row = CorePreviewLimits::build()
			.tenant(input.tenant.clone())
			.idle_seconds(limits.idle_seconds)
			.absolute_seconds(limits.absolute_seconds)
			.revision(revision)
			.updated_at(chrono::Utc::now())
			.finish();
		CorePreviewLimits::objects()
			.save_with_executor(&mut *tx, &row)
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
	} else {
		let row = CorePreviewLimits::build()
			.tenant(input.tenant.clone())
			.idle_seconds(limits.idle_seconds)
			.absolute_seconds(limits.absolute_seconds)
			.revision(revision)
			.updated_at(chrono::Utc::now())
			.finish();
		CorePreviewLimits::objects()
			.insert_with_executor(&mut *tx, &row)
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
	}
	actions.record_denial(&f.store.pool).await?;
	tx.commit().await?;
	Ok(TenantPreviewLimits {
		tenant: input.tenant,
		idle_seconds: limits.idle_seconds,
		absolute_seconds: limits.absolute_seconds,
		revision,
		ceiling_idle_seconds: ceilings.idle_seconds,
		ceiling_absolute_seconds: ceilings.absolute_seconds,
	})
}
