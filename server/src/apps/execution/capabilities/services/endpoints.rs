use super::{contracts::*, management::*, service, sessions};
use crate::{
	Error, Result,
	authorization::{access::Access, execution, identity::Actor},
	domain::{NewTask, Run},
	federation::Federation,
	registry::EntityRef,
};
use reinhardt::query::{Alias, Expr, LockType, Order, PostgresQueryBuilder};
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
	let mut access = access(&f, actor).await?;
	let result = super::configuration::configure(&f.store, &mut access, id, input).await;
	typed(access.finish(result).await?)
}
async fn run_access(f: &Federation, actor: Actor, id: Uuid) -> Result<(Access, Run)> {
	let mut access = access(f, actor).await?;
	let run = access.run_for_interaction(id).await?;
	execution::inherit_run_authority(&mut access, &run).await?;
	let config = service::settings(&mut access, &run).await?;
	if config.core_capabilities.enabled() {
		sessions::initialize(&f.store, &mut access, &run, &config).await?;
	}
	Ok((access, run))
}

pub(crate) async fn areas(
	f: Federation,
	actor: Actor,
	QueryInput(page): QueryInput<AreaQuery>,
) -> Result<AreaPage> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let rows: Vec<Area> = { let query_bind_1 = &access.identity.tenant; let query_bind_2 = &access.identity.subject; let query_bind_3 = page.cursor.unwrap_or(Uuid::nil()); let query_bind_4 = page.thread_id; let query_bind_5 = page.workspace_id; crate::database::native::query_as(&sessions::select("core_areas")
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(query_bind_1.to_owned())))
				.and_where(Expr::col(Alias::new("owner")).eq(Expr::value(query_bind_2.to_owned())))
				.and_where(Expr::col(Alias::new("id")).gt(Expr::value(query_bind_3.to_owned())))
				.and_where(SimpleExpr::CustomWithExpr("((?::uuid IS NULL OR thread_id = ?) AND (?::uuid IS NULL OR workspace_id = ?))".into(), vec![Expr::value(query_bind_4.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into(), Expr::value(query_bind_5.to_owned()).into(), Expr::value(query_bind_5.to_owned()).into()]))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(51)
				.to_string(PostgresQueryBuilder))
		.fetch_all(&mut **access.tx)
		.await? };
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
	input: FileRead,
) -> Result<Envelope> {
	dispatch(f, actor, id, "file_read", json!(input)).await
}
pub(crate) async fn file_search(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: FileSearch,
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
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result = service::invoke(
		&f.store,
		&mut access,
		&run,
		name,
		input,
		&Uuid::new_v4().to_string(),
	)
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
pub(crate) async fn materialize(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: Materialize,
) -> Result<MaterializedFile> {
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result = service::materialize(&f.store, &mut access, &run, input).await;
	typed(access.finish(result).await?)
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
pub(crate) async fn enqueue(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: Enqueue,
) -> Result<SessionStatus> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let area = sessions::load(&mut access, id).await?;
		if !matches!(area.state.as_str(), "active" | "running") {
			return Err(Error::Conflict("AREA_UNAVAILABLE".into()));
		}
		let digest = crate::registry::digest(&json!(["enqueue", id, input]));
		if let Some(result) = sessions::cached(&mut access, input.idempotency_key, &digest).await? {
			return Ok(serde_json::from_value(result)?);
		}
		if sessions::status(&mut access, &area).await?.queue.len() >= 100 {
			return Err(Error::Conflict("QUEUE_LIMIT".into()));
		}
		let workspace = access.workspace(area.workspace_id).await?;
		access.require(&workspace, "task.create").await?;
		let creator = access.identity.subject.clone();
		let task = f
			.store
			.create_task_in(
				&mut access.tx,
				area.workspace_id,
				&NewTask {
					title: input.title,
					description: input.description,
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				&creator,
				None,
			)
			.await?;
		sessions::bind_task(&mut access, task.id, area.thread_id).await?;
		execution::delegate_in(
			&f,
			&mut access,
			task.id,
			&EntityRef {
				id: area.agent_id.clone(),
				version: input.agent_version,
			},
		)
		.await?;
		let result = sessions::status(&mut access, &area).await?;
		if result.queue.is_empty()
			|| !result
				.queue
				.iter()
				.any(|item| item.sequence == area.next_sequence)
		{
			return Err(Error::Invalid(
				"follow-up Agent version must enable core capabilities".into(),
			));
		}
		sessions::cache(&mut access, input.idempotency_key, &digest, &json!(result)).await?;
		Ok(result)
	}
	.await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(result)
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
				&access.resource("workspace", area.workspace_id, json!({})),
				"message.create",
			)
			.await?;
		let limit = f.run_message_limit(&run).await?;
		let sender = access.identity.subject.clone();
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
	let run = f.store.run(result.accepted_run_id).await?;
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

pub(crate) async fn file_share(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::sharing::Share,
) -> Result<Envelope> {
	dispatch(f, actor, id, "file_share", json!(input)).await
}

pub(crate) async fn thread_run(
	f: Federation,
	actor: Actor,
	(workspace, thread, agent): (Uuid, Uuid, String),
	input: Enqueue,
) -> Result<Area> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let channel: crate::apps::workspaces::models::ChannelThread = {
			let query_bind_1 = thread;
			let query_bind_2 = workspace;
			crate::database::query_as(
				&sessions::select("channel_threads")
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							)),
					)
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
		}
		.ok_or_else(|| Error::NotFound("thread unavailable".into()))?;
		let channel = crate::collaboration::ChannelThread::from(channel);
		// Thread deletion takes this same row lock before writing its tombstone.
		// Recheck visibility after acquiring it so a run cannot commit behind a
		// delete that already passed its final area page.
		super::thread_lifecycle::visible(&mut access.tx, thread).await?;
		let digest =
			crate::registry::digest(&json!(["thread_run", workspace, thread, agent, input]));
		if let Some(previous) =
			sessions::cached(&mut access, input.idempotency_key, &digest).await?
		{
			let id = serde_json::from_value(previous["area_id"].clone())?;
			return sessions::load(&mut access, id).await;
		}
		access
			.workspace_record(workspace, "message", channel.root_message_id)
			.await?;
		let resource = access.workspace(workspace).await?;
		access.require(&resource, "task.create").await?;
		let creator = access.identity.subject.clone();
		let task = f
			.store
			.create_task_in(
				&mut access.tx,
				workspace,
				&NewTask {
					title: input.title,
					description: input.description,
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				&creator,
				None,
			)
			.await?;
		sessions::bind_task(&mut access, task.id, thread).await?;
		let delegation = execution::delegate_in(
			&f,
			&mut access,
			task.id,
			&EntityRef {
				id: agent,
				version: input.agent_version,
			},
		)
		.await?;
		let run: Run = {
			let query_bind_1 = task.id;
			aidash_server::database::query_as(
				&sessions::select("runs")
					.and_where(
						Expr::col(Alias::new("task_id")).eq(Expr::value(query_bind_1.to_owned())),
					)
					.limit(1)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **access.tx)
			.await?
		};
		let _ = delegation;
		let area = sessions::for_run(&mut access, &run).await?;
		sessions::cache(
			&mut access,
			input.idempotency_key,
			&digest,
			&json!({"area_id":area.id}),
		)
		.await?;
		Ok(area)
	}
	.await;
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
pub(crate) async fn approval_list(
	f: Federation,
	actor: Actor,
	QueryInput(page): QueryInput<Page>,
) -> Result<ApprovalPage> {
	let mut access = access(&f, actor).await?;
	let result=async {
        let rows:Vec<super::records::Record>={ let query_bind_1 = &access.identity.tenant; let query_bind_2 = &access.identity.subject; let query_bind_3 = page.cursor.unwrap_or(Uuid::nil()); crate::database::native::query_as(&sessions::select("core_records")
            .and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(query_bind_1.to_owned()))).and_where(Expr::col(Alias::new("kind")).is_in(["outbound","grant"]))
            .and_where(SimpleExpr::CustomWithExpr("(owner = ? OR data->>'approver' = ?)".into(), vec![Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
            .and_where(Expr::col(Alias::new("id")).gt(Expr::value(query_bind_3.to_owned()))).order_by(Alias::new("id"),Order::Asc).limit(51).to_string(PostgresQueryBuilder)).fetch_all(&mut **access.tx).await? };
        let next_cursor=(rows.len()==51).then(||rows[49].id);
        let mut items=vec![];
        for row in rows.into_iter().take(50) {
			if !super::approvals::visible(&mut access, &row).await? { continue; }
            let run:Uuid=serde_json::from_value(row.data["run_id"].clone())?;
            // Designated approvers see only the scoped card, never Run output.
            items.push(json!({"id":row.id,"kind":row.kind,"run_id":run,"area_id":row.area_id,"state":if row.state=="pending"&&row.expires_at.is_some_and(|t|t<chrono::Utc::now()){"expired"}else{&row.state},"revision":row.revision,"requester":row.owner,"approver":row.data["approver"],"targets":row.data["targets"],"action":"network.get","expires_at":row.expires_at,"grant_id":row.data["grant_id"]}));
        }
        Ok(json!({"items":items,"next_cursor":next_cursor}))
    }.await;
	typed(access.finish(result).await?)
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
	let result = super::approvals::revoke(&mut access, id, "outbound", input).await;
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
pub(crate) async fn python_install(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::packages::Install,
) -> Result<Envelope> {
	dispatch(f, actor, id, "python_install", json!(input)).await
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
		service::available(&area)?;
		let entry = if let Some(entry) = service::files(&area)?
			.into_iter()
			.find(|entry| entry.file_id == file)
		{
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
			let (digest, size, kind) =
				meta.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
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
		};
		download(&f.store, &mut access, entry, input.offset.unwrap_or(0)).await
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn managed_areas(
	f: Federation,
	actor: Actor,
	QueryInput(input): QueryInput<Page>,
) -> Result<super::cleanup::ManagementPage> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::inventory(&mut access, input.cursor).await;
	access.finish(result).await
}
pub(crate) async fn cleanup(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::cleanup::Cleanup,
) -> Result<super::cleanup::CleanupResult> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::prepare(&f.store, &mut access, id, input).await;
	access.finish(result).await
}
pub(crate) async fn cleanup_status(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<super::cleanup::CleanupResult> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::status(&mut access, id).await;
	access.finish(result).await
}
pub(crate) async fn cleanup_reconcile(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<super::cleanup::CleanupResult> {
	let mut access = access(&f, actor.clone()).await?;
	let result = async {
		super::cleanup::status(&mut access, id).await?;
		super::records::get(&mut access, id, "cleanup").await
	}
	.await;
	let record = access.finish(result).await?;
	// Reconcile a previously authorized, committed deletion intent. Release the
	// read transaction first; the job takes its own generation/ownership locks.
	super::cleanup::erase_job(&f.store, record).await?;
	cleanup_status(f, actor, id).await
}
pub(crate) async fn deletion_confirmation(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: ExpectedRevision,
) -> Result<DeletionConfirmation> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::confirmation(&mut access, id, input.expected_revision).await;
	typed(access.finish(result).await?)
}
pub(crate) async fn restore(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: super::cleanup::Restore,
) -> Result<Area> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::restore(&f.store, &mut access, id, input).await;
	access.finish(result).await
}

pub(crate) async fn restore_new_thread(
	f: Federation,
	actor: Actor,
	(workspace, id): (Uuid, Uuid),
	input: super::cleanup::RestoreNewThread,
) -> Result<Value> {
	let mut lease =
		crate::collaboration::access::Lease::begin_message_create(&f.store, actor, workspace)
			.await?;
	let result = async {
		// Restore takes the sharing lock before publishing events. Acquire it
		// before the new message does too, so concurrent restores cannot deadlock.
		super::sharing::serialize(lease.access_mut().ok_or(Error::Forbidden)?).await?;
		let message = crate::collaboration::threads::post(
			&f.store,
			&mut lease,
			workspace,
			crate::collaboration::ChannelMessageInput {
				content: input.content,
				thread_id: None,
				idempotency_key: input.idempotency_key,
				attachment_ids: vec![],
			},
		)
		.await?;
		let thread = crate::collaboration::threads::create(
			&f.store,
			&mut lease,
			workspace,
			message.message.id,
		)
		.await?;
		let access = lease.access_mut().ok_or(Error::Forbidden)?;
		let area = super::cleanup::restore(
			&f.store,
			access,
			id,
			super::cleanup::Restore {
				idempotency_key: input.idempotency_key,
				expected_revision: input.expected_revision,
				snapshot_id: input.snapshot_id,
				thread_id: thread.id,
			},
		)
		.await?;
		Ok(json!({"thread":thread,"area":area}))
	}
	.await;
	lease.finish(result).await
}

pub(crate) async fn transfer_status(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<TransferStatus> {
	let mut access = access(&f, actor).await?;
	let result = super::transfer::view(&mut access, id).await;
	typed(access.finish(result).await?)
}
pub(crate) async fn transfer_history(
	f: Federation,
	actor: Actor,
	id: Uuid,
	QueryInput(page): QueryInput<Page>,
) -> Result<TransferPage> {
	let mut access = access(&f, actor).await?;
	let result = async {
		super::cleanup::load(&mut access, id, "file.read").await?;
		let records: Vec<super::records::Record> = {
			let query_bind_1 = id;
			let query_bind_2 = &access.identity.tenant;
			let query_bind_3 = &access.identity.subject;
			let query_bind_4 = page.cursor.unwrap_or(Uuid::nil());
			crate::database::native::query_as(
				&sessions::select("core_records")
					.and_where(
						Expr::col(Alias::new("area_id")).eq(Expr::value(query_bind_1.to_owned())),
					)
					.and_where(
						Expr::col(Alias::new("tenant")).eq(Expr::value(query_bind_2.to_owned())),
					)
					.and_where(
						Expr::col(Alias::new("owner")).eq(Expr::value(query_bind_3.to_owned())),
					)
					.and_where(
						Expr::col(Alias::new("kind")).is_in(["transfer_out", "collaboration"]),
					)
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
			items.push(if record.kind == "transfer_out" {
				super::transfer::view(&mut access, record.id).await?
			} else {
				record.data
			});
		}
		Ok(json!({"items":items,"next_cursor":next_cursor}))
	}
	.await;
	typed(access.finish(result).await?)
}
pub(crate) async fn transfer_reconcile(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<TransferStatus> {
	let mut access = access(&f, actor.clone()).await?;
	let result = super::transfer::view(&mut access, id).await;
	access.finish(result).await?;
	super::transfer::reconcile(&f, id).await?;
	transfer_status(f, actor, id).await
}

pub(crate) async fn transfer_recipients(
	f: Federation,
	actor: Actor,
	QueryInput(input): QueryInput<RecipientQuery>,
) -> Result<RecipientPage> {
	let mut access = access(&f, actor).await?;
	let result = async {
		if input.node_id == f.config.node_id {
			return super::transfer::recipient_list(&f, &mut access, input.cursor).await;
		}
		access
			.require(
				&access.resource("node", &input.node_id, json!({})),
				"file.transfer",
			)
			.await?;
		crate::authorization::peer::authority_request(
			&f,
			&input.node_id,
			"/scoped/files/recipients",
			&json!(super::transfer::Requester {
				tenant: access.identity.tenant.clone(),
				subject: access.identity.subject.clone(),
				cursor: input.cursor
			}),
		)
		.await
	}
	.await;
	typed(access.finish(result).await?)
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
	(workspace, thread): (Uuid, Uuid),
	input: super::thread_lifecycle::DeleteThread,
) -> Result<DeletedThread> {
	let mut access = access(&f, actor).await?;
	let result =
		super::thread_lifecycle::delete(&f.store, &mut access, workspace, thread, input).await;
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
	AreaQuery, DownloadChunk, ExpectedRevision, Offset, OutboundPage, Page, RecipientQuery,
	ReferencePage,
};

use reinhardt::query::SimpleExpr;
