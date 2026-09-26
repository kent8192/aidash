//! Durable operation admission and reconciliation. Workers never run host commands.
use super::{contracts::*, objects, service, sessions};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	domain::Run,
	store::Store,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use sea_orm::sea_query::{Alias, Expr, LockType, Order, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, sqlx::FromRow)]
pub(crate) struct Operation {
	pub id: Uuid,
	pub area_id: Uuid,
	pub run_id: Uuid,
	pub tenant: String,
	pub principal: String,
	pub credential_id: Uuid,
	pub subjects: Value,
	pub digest: String,
	pub kind: String,
	pub state: String,
	pub epoch: i64,
	pub generation: i64,
	pub revision: i64,
	pub policy_revision: i64,
	pub input: Value,
	pub result: Value,
	pub runner_instance: Option<String>,
}

pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Shell,
	_key: &str,
) -> Result<Value> {
	prepare_kind(store, access, run, area, input, "shell", json!({})).await
}
pub(crate) async fn prepare_kind(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Shell,
	kind: &str,
	extra: Value,
) -> Result<Value> {
	let seconds = input
		.timeout_seconds
		.unwrap_or(store.capabilities.0.operation_seconds);
	if input.command.is_empty()
		|| input.command.len() > store.capabilities.0.limits.command_bytes
		|| seconds == 0
		|| seconds > store.capabilities.0.maximum_seconds
	{
		return Err(Error::Invalid("INVALID_SHELL_LIMIT".into()));
	}
	let digest = crate::registry::digest(&json!([kind, run.id, input, extra]));
	let key = format!("core:{}:{}", run.id, input.idempotency_key);
	let previous: Option<Operation> = sqlx::query_as(
		&sessions::select("core_operations")
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("principal")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("request_key")).eq(Expr::cust("$3")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(&access.identity.subject)
	.bind(&key)
	.fetch_optional(&mut **access.tx)
	.await?;
	if let Some(previous) = previous {
		if previous.digest != digest {
			return Err(Error::Conflict("IDEMPOTENCY_CONFLICT".into()));
		}
		return Ok(json!(result(store, access, &previous, 0).await?));
	}
	service::available(area)?;
	if !store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	if sessions::status(access, area).await?.active_run_id != Some(run.id)
		|| matches!(run.phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED")
	{
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	sessions::authorize(access, area, "file.write").await?;
	verified_health(store, kind == "code_interpreter").await?;
	if matches!(kind, "shell" | "python_install") {
		super::python::release(
			store,
			access,
			area,
			if kind == "python_install" {
				"packages_changed"
			} else {
				"shell_requested"
			},
		)
		.await?;
	}
	let id = Uuid::new_v4();
	area.epoch += 1;
	set_area(access, area.id, "running", area.epoch).await?;
	let mut value = json!({"command":input.command,"seconds":seconds});
	value
		.as_object_mut()
		.unwrap()
		.extend(extra.as_object().ok_or(Error::Forbidden)?.clone());
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_operations"))
			.columns(
				[
					"id",
					"area_id",
					"run_id",
					"tenant",
					"principal",
					"credential_id",
					"subjects",
					"request_key",
					"digest",
					"kind",
					"state",
					"epoch",
					"generation",
					"revision",
					"policy_revision",
					"input",
					"result",
				]
				.map(Alias::new),
			)
			.values_panic((1..=17).map(|i| Expr::cust(format!("${i}"))))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(area.id)
	.bind(run.id)
	.bind(&access.identity.tenant)
	.bind(&access.identity.subject)
	.bind(access.identity.credential_id)
	.bind(json!(access.subjects))
	.bind(&key)
	.bind(&digest)
	.bind(kind)
	.bind("prepared")
	.bind(area.epoch)
	.bind(area.generation)
	.bind(area.revision)
	.bind(access.snapshot.revision)
	.bind(value)
	.bind(json!({}))
	.execute(&mut **access.tx)
	.await?;
	store
		.event(
			&mut access.tx,
			Some(area.workspace_id),
			"capability.operation_accepted",
			json!({"area_id":area.id,"operation_id":id,"epoch":area.epoch}),
		)
		.await?;
	let operation = get(access, id).await?;
	Ok(json!(result(store, access, &operation, 0).await?))
}

pub(crate) async fn get(access: &mut Access, id: Uuid) -> Result<Operation> {
	sqlx::query_as(
		&sessions::select("core_operations")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(&access.identity.tenant)
	.fetch_optional(&mut **access.tx)
	.await?
	.ok_or_else(|| Error::NotFound("operation unavailable".into()))
}

pub(crate) async fn poll(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	input: OperationInput,
	kind: &str,
	cancel: bool,
) -> Result<Value> {
	let mut operation = get(access, input.operation_id).await?;
	if operation.area_id != area.id
		|| operation.run_id != run.id
		|| operation.principal != access.identity.subject
		|| operation.kind != kind
			&& !(kind == "code_interpreter" && operation.kind == "python_install")
	{
		return Err(Error::NotFound("operation unavailable".into()));
	}
	if cancel
		&& !matches!(
			operation.state.as_str(),
			"completed" | "cancelled" | "failed" | "withdrawn"
		) {
		if operation.state == "prepared" {
			operation.state = "cancelled".into();
			operation.result =
				json!({"termination_confirmed":true,"effects_may_have_occurred":false});
			set_area(access, area.id, "active", area.epoch).await?;
			if operation.kind == "code_interpreter" {
				super::python::completed(access, area, &operation, &json!({"writer_frozen":false}))
					.await?;
			}
		} else {
			operation.state = "cancelling".into();
		}
		persist(access, &operation).await?;
	}
	Ok(json!(
		result(store, access, &operation, input.offset.unwrap_or(0)).await?
	))
}

pub(crate) async fn result(
	store: &Store,
	access: &mut Access,
	operation: &Operation,
	offset: usize,
) -> Result<OperationResult> {
	let mut output = operation.result["preview"]
		.as_str()
		.unwrap_or_default()
		.to_owned();
	let mut next_offset = None;
	if let Some(file) = operation.result.get("output_file") {
		let entry: FileEntry = serde_json::from_value(file.clone())?;
		let bytes = store.capabilities.read(access, &entry).await?;
		let text = String::from_utf8_lossy(&bytes);
		if offset > text.len() || !text.is_char_boundary(offset) {
			return Err(Error::Invalid("INVALID_READ_RANGE".into()));
		}
		let mut end = text
			.len()
			.min(offset.saturating_add(store.capabilities.0.limits.read_bytes));
		while !text.is_char_boundary(end) {
			end -= 1;
		}
		output = text[offset..end].into();
		next_offset = (end < text.len()).then_some(end);
	}
	Ok(OperationResult {
		operation_id: operation.id,
		kind: operation.kind.clone(),
		status: operation.state.clone(),
		area_id: operation.area_id,
		generation: operation.generation,
		revision: operation.revision,
		epoch: operation.epoch,
		policy_revision: operation.policy_revision,
		termination_confirmed: operation.result["termination_confirmed"] == true,
		writer_frozen: operation.result["writer_frozen"] == true,
		session_id: serde_json::from_value(
			operation
				.input
				.get("session_id")
				.cloned()
				.unwrap_or(Value::Null),
		)?,
		displays: serde_json::from_value(
			operation
				.result
				.get("displays")
				.cloned()
				.unwrap_or(json!([])),
		)?,
		exit_code: operation.result["exit_code"].as_i64(),
		output,
		next_offset,
		truncated: operation.result["truncated"] == true,
		effects_may_have_occurred: operation.state != "prepared"
			&& operation.result["effects_may_have_occurred"] != false,
		error: super::errors::CapabilityError::stored(&operation.result["error"]),
	})
}

pub(crate) async fn set_area(access: &mut Access, id: Uuid, state: &str, epoch: i64) -> Result<()> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_areas"))
			.value(Alias::new("state"), Expr::cust("$2"))
			.value(Alias::new("epoch"), Expr::cust("$3"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(state)
	.bind(epoch)
	.execute(&mut **access.tx)
	.await?;
	Ok(())
}

async fn persist(access: &mut Access, operation: &Operation) -> Result<()> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.values([
				(Alias::new("state"), Expr::cust("$2")),
				(Alias::new("result"), Expr::cust("$3")),
				(Alias::new("runner_instance"), Expr::cust("$4")),
				(Alias::new("revision"), Expr::cust("$5")),
				(Alias::new("updated_at"), Expr::current_timestamp().into()),
			])
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(operation.id)
	.bind(&operation.state)
	.bind(&operation.result)
	.bind(&operation.runner_instance)
	.bind(operation.revision)
	.execute(&mut **access.tx)
	.await?;
	Ok(())
}

fn client(store: &Store) -> Result<(reqwest::Client, String, String)> {
	let config = store.capabilities.0.runner.as_ref().ok_or_else(|| {
		Error::Conflict("RUNTIME_UNAVAILABLE: no isolated runner configured".into())
	})?;
	let url = reqwest::Url::parse(&config.endpoint)
		.map_err(|_| Error::Invalid("invalid runner endpoint".into()))?;
	if url.scheme() != "https"
		&& !(url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "::1")))
	{
		return Err(Error::Invalid(
			"runner transport requires TLS or loopback".into(),
		));
	}
	let token = std::env::var(&config.token_env).map_err(|_| {
		Error::Conflict("RUNTIME_UNAVAILABLE: runner credential unavailable".into())
	})?;
	let client = reqwest::Client::builder()
		.timeout(std::time::Duration::from_secs(10))
		.redirect(reqwest::redirect::Policy::none())
		.build()?;
	Ok((client, config.endpoint.trim_end_matches('/').into(), token))
}

pub(crate) async fn verified_health(store: &Store, python: bool) -> Result<Value> {
	let p = &store.capabilities.0;
	let runner = p.runner.as_ref().ok_or_else(|| {
		Error::Conflict("RUNTIME_UNAVAILABLE: configure the isolated runner profile".into())
	})?;
	let health = remote(store, reqwest::Method::GET, "/v1/health", None)
		.await
		.map_err(|_| {
			Error::Conflict(
				"RUNTIME_UNAVAILABLE: start the runner and pass its deployment probes".into(),
			)
		})?;
	let limits = &health["probe"]["resources"];
	if health["protocol"] != "aidash-runner/1"
		|| health["verified"] != true
		|| health["image"] != runner.image
		|| health["runtime_class"] != runner.runtime_class
		|| (python && health["python_verified"] != true)
		|| limits["cpu"].as_f64() != Some(p.cpu as f64)
		|| limits["memory_bytes"] != p.memory_bytes
		|| limits["swap_bytes"] != 0
		|| limits["processes"] != p.processes
		|| limits["working_bytes"] != p.working_bytes
		|| limits["temporary_bytes"] != p.temporary_bytes
	{
		return Err(Error::Conflict(
			"RUNTIME_UNAVAILABLE: deployment probes do not match the execution profile".into(),
		));
	}
	Ok(health)
}

pub(crate) async fn remote(
	store: &Store,
	method: reqwest::Method,
	path: &str,
	body: Option<Value>,
) -> Result<Value> {
	let (client, endpoint, token) = client(store)?;
	let mut request = client
		.request(method, format!("{endpoint}{path}"))
		.bearer_auth(token);
	if let Some(body) = body {
		request = request.json(&body);
	}
	let response = request.send().await?;
	let status = response.status();
	let value: Value = response.json().await?;
	if status == reqwest::StatusCode::NOT_FOUND && path.starts_with("/v1/operations/") {
		return Ok(json!({"status":"absent"}));
	}
	if !status.is_success() {
		return Err(Error::External(format!(
			"runner {}: {}",
			status.as_u16(),
			value["error"]
		)));
	}
	Ok(value)
}

async fn drive(store: &Store, id: Uuid) -> Result<()> {
	let snapshot: Operation = sqlx::query_as(
		&sessions::select("core_operations")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_one(&store.pool)
	.await?;
	let identity = SubjectIdentity {
		credential_id: snapshot.credential_id,
		tenant: snapshot.tenant.clone(),
		subject: snapshot.principal.clone(),
	};
	let mut access = match Access::begin(store, &identity).await {
		Ok(access) => access,
		Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => {
			return withdraw(store, &snapshot).await;
		}
		Err(error) => return Err(error),
	};
	access.subjects = serde_json::from_value(snapshot.subjects.clone())?;
	let outcome = Box::pin(async {
		let run = access.run_for_interaction(snapshot.run_id).await?;
		let mut area = sessions::for_run(&mut access, &run).await?;
		let mut operation = get(&mut access, id).await?;
		if !matches!(operation.state.as_str(), "prepared" | "submitted" | "running" | "cancelling") { return Ok(()); }
		if area.epoch != operation.epoch || area.generation != operation.generation || area.revision != operation.revision {
			return Err(Error::Forbidden);
		}
		let config = service::settings(&mut access, &run).await?;
		if !config.core_capabilities.permits(&operation.kind) { return Err(Error::Forbidden); }
		access.require(&access.resource("tool", format!("builtin:{}", operation.kind), json!({})), "tool.invoke").await?;
		if operation.kind == "python_install" {
			let request: super::packages::Install = serde_json::from_value(operation.input["package_request"].clone())?;
			super::packages::authorize(store, &mut access, &run, &area, &request.wheels).await?;
		}
		let health = verified_health(store, operation.kind == "code_interpreter").await?;
		let instance = health["instance"].as_str().ok_or_else(|| Error::External("invalid runner identity".into()))?;
		if operation.runner_instance.as_deref().is_some_and(|old| old != instance) {
			operation.state = "uncertain".into(); operation.result = json!({"error":"runner journal identity changed"});
			set_area(&mut access, area.id, "uncertain", area.epoch).await?;
			return persist(&mut access, &operation).await;
		}
		let cancelling = !store.capabilities.0.admission || operation.state == "cancelling" || run.control == "CANCELLED" || matches!(run.phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED");
		if cancelling && operation.state == "prepared" {
			operation.state = "cancelled".into();
			operation.result = json!({"termination_confirmed":true,"effects_may_have_occurred":false});
			set_area(&mut access, area.id, "active", area.epoch).await?;
			if operation.kind == "code_interpreter" { super::python::completed(&mut access,&area,&operation,&json!({"writer_frozen":false})).await?; }
			return persist(&mut access, &operation).await;
		}
		if operation.state == "prepared" {
			operation.runner_instance = Some(instance.into());
			operation.state = "submitted".into();
			persist(&mut access, &operation).await?;
			// Commit the attempted operation before handing it to another process.
			// The next pass either observes the UUID or submits the recorded bytes.
			operation.result = json!({"dispatch_pending":true});
			return persist(&mut access, &operation).await;
		}
		let observed = if cancelling {
			operation.state = "cancelling".into();
			remote(store, reqwest::Method::POST, &format!("/v1/operations/{id}/cancel"), None).await?
		} else if operation.result["dispatch_pending"] == true {
			let mut files = vec![];
			for file in service::files(&area)?.into_iter().chain(super::skills::mounted(&mut access,run.id).await?).chain(super::packages::inputs(&operation)?) {
				files.push(json!({"file_id":file.file_id,"path":file.path,"digest":file.digest,"scope":file.scope,"size":file.size}));
			}
			let mut input = json!({"operation_id":operation.id,"area_id":area.id,"epoch":operation.epoch,"digest":operation.digest,
				"kind":if operation.kind=="code_interpreter"{"python"}else{"shell"},"code":operation.input["command"],"seconds":operation.input["seconds"],"files":files});
			if operation.kind=="code_interpreter" {input["session_id"]=operation.input["session_id"].clone();}
            remote(store, reqwest::Method::POST, "/v1/operations", Some(input)).await?
		} else { remote(store, reqwest::Method::GET, &format!("/v1/operations/{id}"), None).await? };
		match observed["status"].as_str() {
			Some("awaiting_files") => {
				Box::pin(upload_inputs(store, &mut access, &area, run.id, &operation)).await?;
				remote(store, reqwest::Method::POST, &format!("/v1/operations/{id}/start"), None).await?;
				operation.state = "submitted".into();
				operation.result = json!({});
			}
			Some("absent") => {
				operation.state = "uncertain".into(); operation.result = json!({"error":"runner lost a previously accepted operation"});
				set_area(&mut access, area.id, "uncertain", area.epoch).await?;
			}
			Some("completed" | "cancelled" | "failed") if observed["termination_confirmed"] == true || operation.kind=="code_interpreter" && observed["writer_frozen"]==true => {
				if observed["executed"] == false {
					operation.state = observed["status"].as_str().unwrap().into();
					operation.result = json!({"termination_confirmed":true,"effects_may_have_occurred":false,"runner_acknowledged":false});
					set_area(&mut access, area.id, "active", area.epoch).await?;
					return persist(&mut access, &operation).await;
				}
				let previous = service::files(&area)?;
                let mut entries = previous.iter().filter(|file| !matches!(file.scope, FileScope::Working)).cloned().collect::<Vec<_>>();
				let exported = observed["files"].as_array().ok_or_else(|| Error::External("invalid runner file manifest".into()))?;
				let mut size = entries.iter().map(|file| file.size).sum::<u64>();
				let mut seen = std::collections::BTreeSet::new();
				for file in exported {
					let path = file["path"].as_str().ok_or(Error::Forbidden)?;
					objects::validate_path(path)?;
					if !seen.insert(path) || entries.len() >= 4096 { return Err(Error::Invalid("invalid runner paths".into())); }
					let file_size = file["size"].as_u64().ok_or(Error::Forbidden)?;
					size = size.checked_add(file_size).ok_or(Error::Forbidden)?;
					if size > store.capabilities.0.working_bytes { return Err(Error::Conflict("runner file quota".into())); }
					if let Some(unchanged) = previous.iter().find(|old| matches!(old.scope,FileScope::Working) && old.path==path && old.size==file_size && file["digest"]==old.digest) {
                        store.capabilities.verified(&mut access,unchanged).await?;
                        entries.push(unchanged.clone());
                        continue;
                    }
                    let (file_id,digest) = Box::pin(download_output(store, &mut access, area.id, id, file)).await?;
					entries.push(FileEntry { file_id, path:path.into(), digest, size:file_size, media_type:"application/octet-stream".into(), scope:FileScope::Working, provenance:json!({"kind":"operation","operation_id":id}) });
				}
				let stdout = STANDARD.decode(observed["stdout"].as_str().ok_or(Error::Forbidden)?).map_err(|_| Error::Invalid("invalid runner output encoding".into()))?;
				if stdout.len() as u64 > store.capabilities.0.output_bytes { return Err(Error::Conflict("runner output quota".into())); }
				let (output_id, output_digest) = store.capabilities.put(&mut access, Some(area.id), "output", &stdout).await?;
				let output = FileEntry {file_id: output_id, path:format!("operation-{id}.log"), digest:output_digest, size:stdout.len() as u64, media_type:"text/plain; charset=utf-8".into(), scope:FileScope::Working, provenance:json!({"kind":"operation","operation_id":id})};
                let mut displays=vec![];let mut display_bytes=0;
                for (index,display) in observed["displays"].as_array().map(Vec::as_slice).unwrap_or_default().iter().enumerate() {
                    if index>=16||display["mime"]!="image/png" {return Err(Error::Invalid("DISPLAY_LIMIT".into()));}
                    let bytes=STANDARD.decode(display["data"].as_str().ok_or(Error::Forbidden)?).map_err(|_|Error::Invalid("INVALID_DISPLAY".into()))?;
                    display_bytes+=bytes.len();if display_bytes as u64>store.capabilities.0.output_bytes||!bytes.starts_with(b"\x89PNG\r\n\x1a\n") {return Err(Error::Invalid("DISPLAY_LIMIT".into()));}
                    let (file_id,digest)=store.capabilities.put(&mut access,Some(area.id),"display",&bytes).await?;
                    displays.push(FileEntry{file_id,path:format!("display-{id}-{index}.png"),digest,size:bytes.len() as u64,media_type:"image/png".into(),scope:FileScope::Working,provenance:json!({"kind":"display","operation_id":id})});
                }
				area.manifest = json!(entries);
				service::publish(store, &mut access, &mut area).await?;
				set_area(&mut access, area.id, "active", area.epoch).await?;
				operation.revision = area.revision;
				operation.state = observed["status"].as_str().unwrap().into();
				operation.result = json!({"displays":displays,"session_id":operation.input["session_id"],"writer_frozen":observed["writer_frozen"],"output_file":output,"exit_code":observed["exit_code"],"truncated":observed["truncated"],"termination_confirmed":observed["termination_confirmed"],"runner_acknowledged":false,"error":observed["error"]});
                if operation.kind=="code_interpreter" {super::python::completed(&mut access,&area,&operation,&observed).await?;}
			}
			Some("uncertain") => {
				operation.state = "uncertain".into(); operation.result = json!({"error":observed["error"],"termination_confirmed":observed["termination_confirmed"]});
				set_area(&mut access, area.id, "uncertain", area.epoch).await?;
			}
			Some("accepted" | "starting" | "running" | "finishing") => {
				if operation.state != "cancelling" { operation.state = if matches!(observed["status"].as_str(), Some("accepted" | "starting")) { "submitted" } else { "running" }.into(); }
				let bytes = STANDARD.decode(observed["stdout"].as_str().unwrap_or_default()).map_err(|_| Error::Invalid("invalid runner output preview".into()))?;
				if bytes.len() > 16384 { return Err(Error::Invalid("runner preview limit".into())); }
				let text = String::from_utf8_lossy(&bytes);
				let mut end = text.len().min(store.capabilities.0.limits.read_bytes);
				while !text.is_char_boundary(end) { end -= 1; }
				operation.result = json!({"preview":&text[..end],"truncated":observed["truncated"]});
			}
			_ => return Err(Error::External("invalid runner operation state".into())),
		}
		persist(&mut access, &operation).await?;
		store.event(&mut access.tx, Some(area.workspace_id), "capability.operation_changed", json!({"operation_id":id,"area_id":area.id,"status":operation.state})).await?;
		Ok(())
	}).await;
	let result = access.finish(outcome).await;
	match result {
		Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => {
			withdraw(store, &snapshot).await
		}
		other => other,
	}
}

async fn withdraw(store: &Store, operation: &Operation) -> Result<()> {
	// Cancellation narrows an already-authorized operation and remains possible
	// after its originating credential or source visibility has been revoked.
	let mut tx = store.pool.begin().await?;
	// Keep the normal area-before-operation lock order and recheck committed
	// state: a stale worker snapshot cannot declare a dispatched writer safe.
	let area: Area = sqlx::query_as(
		&sessions::select("core_areas")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(operation.area_id)
	.fetch_one(&mut *tx)
	.await?;
	let operation: Operation = sqlx::query_as(
		&sessions::select("core_operations")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(operation.id)
	.fetch_one(&mut *tx)
	.await?;
	if !matches!(
		operation.state.as_str(),
		"prepared" | "submitted" | "running" | "cancelling"
	) {
		return Ok(());
	}
	let never_dispatched = operation.state == "prepared";
	let observed = if never_dispatched {
		json!({"termination_confirmed":true})
	} else {
		remote(
			store,
			reqwest::Method::POST,
			&format!("/v1/operations/{}/cancel", operation.id),
			None,
		)
		.await?
	};
	let stopped = observed["termination_confirmed"] == true;
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.value(Alias::new("state"), Expr::cust("$2"))
			.value(Alias::new("result"), Expr::cust("$3"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(operation.id)
	.bind(if stopped { "withdrawn" } else { "cancelling" })
	.bind(json!({"termination_confirmed":stopped,"effects_may_have_occurred":!never_dispatched,"error":"AUTHORITY_WITHDRAWN"}))
	.execute(&mut *tx)
	.await?;
	if area.epoch == operation.epoch
		&& area.generation == operation.generation
		&& area.state == "running"
	{
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_areas"))
				.value(
					Alias::new("state"),
					Expr::val(if never_dispatched {
						"active"
					} else {
						"uncertain"
					}),
				)
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(operation.area_id)
		.execute(&mut *tx)
		.await?;
	}
	tx.commit().await?;
	Ok(())
}

async fn upload_inputs(
	store: &Store,
	access: &mut Access,
	area: &Area,
	run: Uuid,
	operation: &Operation,
) -> Result<()> {
	let id = operation.id;
	for file in service::files(area)?
		.into_iter()
		.chain(super::skills::mounted(access, run).await?)
		.chain(super::packages::inputs(operation)?)
	{
		let mut offset = 0;
		loop {
			let bytes = store.capabilities.read_chunk(access, &file, offset).await?;
			let length = bytes.len() as u64;
			if length == 0 && offset < file.size {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			remote(
				store,
				reqwest::Method::POST,
				&format!(
					"/v1/operations/{id}/inputs/{}?offset={offset}",
					file.file_id
				),
				Some(json!({"data":STANDARD.encode(bytes)})),
			)
			.await?;
			offset += length;
			if offset == file.size {
				break;
			}
		}
	}
	Ok(())
}

async fn download_output(
	store: &Store,
	access: &mut Access,
	area: Uuid,
	operation: Uuid,
	file: &Value,
) -> Result<(Uuid, String)> {
	let size = file["size"]
		.as_u64()
		.ok_or_else(|| Error::Invalid("invalid output size".into()))?;
	let id = file["object_id"]
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid output id".into()))?;
	if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return Err(Error::Invalid("invalid output id".into()));
	}
	let expected = file["digest"]
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid output digest".into()))?;
	let mut object = store
		.capabilities
		.begin_object(access, Some(area), "working", size)
		.await?;
	let mut offset = 0;
	while offset < size {
		let chunk = remote(
			store,
			reqwest::Method::GET,
			&format!("/v1/operations/{operation}/files/{id}?offset={offset}"),
			None,
		)
		.await?;
		let bytes = STANDARD
			.decode(chunk["data"].as_str().ok_or(Error::Forbidden)?)
			.map_err(|_| Error::Invalid("invalid output chunk".into()))?;
		if bytes.is_empty()
			|| bytes.len() > 4 << 20
			|| chunk["offset"] != offset
			|| chunk["size"] != size
			|| chunk["digest"] != expected
		{
			return Err(Error::Conflict("output chunk integrity".into()));
		}
		offset += bytes.len() as u64;
		object.write_block(&bytes).await?;
	}
	object.finish(access, Some(expected)).await
}

async fn acknowledge_results(store: &Store, cursor: &mut Uuid) -> Result<()> {
	let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
		&Query::select()
			.columns(["id", "digest", "runner_instance"].map(Alias::new))
			.from(Alias::new("core_operations"))
			.and_where(Expr::cust("result->'runner_acknowledged' = 'false'::jsonb"))
			.and_where(Expr::col(Alias::new("state")).is_in(["completed", "cancelled", "failed"]))
			.and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$1")))
			.order_by(Alias::new("id"), sea_orm::sea_query::Order::Asc)
			.limit(16)
			.to_string(PostgresQueryBuilder),
	)
	.bind(*cursor)
	.fetch_all(&store.pool)
	.await?;
	if rows.is_empty() {
		*cursor = Uuid::nil();
	}
	for (id, digest, instance) in rows {
		*cursor = id;
		let health = remote(store, reqwest::Method::GET, "/v1/health", None).await?;
		let lost = instance.as_deref().is_some_and(|old| {
			health["instance"]
				.as_str()
				.is_some_and(|current| current != old)
		});
		if !lost
			&& remote(
				store,
				reqwest::Method::POST,
				&format!("/v1/operations/{id}/ack"),
				Some(json!({"digest":digest})),
			)
			.await
			.is_err()
		{
			continue;
		}
		// Completed data is already committed. A different durable runner
		// identity proves its old journal cannot be acknowledged here.
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_operations"))
				.value(
					Alias::new("result"),
					Expr::cust("jsonb_set(result, '{runner_acknowledged}', 'true'::jsonb)"),
				)
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.execute(&store.pool)
		.await?;
	}
	Ok(())
}

pub async fn run(store: Store, stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	tokio::try_join!(
		Box::pin(execution_loop(store.clone(), stopping.clone())),
		Box::pin(super::network::run(store.clone(), stopping.clone())),
		Box::pin(super::references::run(store.clone(), stopping.clone())),
		Box::pin(super::cleanup::run(store.clone(), stopping.clone())),
		Box::pin(super::reclamation::run(store, stopping))
	)?;
	Ok(())
}
async fn execution_loop(
	store: Store,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	let mut acknowledgement_cursor = Uuid::nil();
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		let ids: Vec<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_operations"))
				.and_where(Expr::col(Alias::new("state")).is_in([
					"prepared",
					"submitted",
					"running",
					"cancelling",
				]))
				.order_by(Alias::new("updated_at"), Order::Asc)
				.limit(16)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&store.pool)
		.await?;
		for id in ids {
			if let Err(error) = Box::pin(drive(&store, id)).await {
				if matches!(&error, Error::Conflict(message) if message.starts_with("STORAGE_QUOTA") || message == "runner file quota" || message == "runner output quota")
				{
					// Publication rolled back. Keep observing the same runner result
					// after capacity is restored, and expose the blocked publication
					// instead of silently polling forever or replaying execution.
					let blocked = json!({"code":"STORAGE_QUOTA","message":"Storage quota prevents saving this result. Restore capacity to reconcile the same operation; prior files remain intact.","retryable":true});
					sqlx::query(
						&Query::update()
							.table(Alias::new("core_operations"))
							.value(
								Alias::new("result"),
								Expr::cust("jsonb_set(result, '{error}', $2::jsonb)"),
							)
							.value(Alias::new("updated_at"), Expr::current_timestamp())
							.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
							.and_where(Expr::col(Alias::new("state")).is_in([
								"prepared",
								"submitted",
								"running",
								"cancelling",
							]))
							.to_string(PostgresQueryBuilder),
					)
					.bind(id)
					.bind(blocked)
					.execute(&store.pool)
					.await?;
				}
				tracing::warn!(%id, %error, "capability operation reconciliation pending");
			}
		}
		if let Err(error) = acknowledge_results(&store, &mut acknowledgement_cursor).await {
			tracing::warn!(%error, "runner receipt acknowledgement pending");
		}
		tokio::select! { _ = stopping.changed() => {}, _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {} }
	}
}
