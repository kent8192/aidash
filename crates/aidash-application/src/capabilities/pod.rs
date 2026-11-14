//! Shell commands and Python cells run in the Execution Pod of the Run's
//! Scratch Space (#745). A tool call only admits the operation; the capability
//! worker starts the Pod, submits the command with the Run credential of
//! ADR 0136 and records what the Runner reports, so no Runner call waits inside
//! an authority transaction. The rows stay in `core_operations`, which keeps
//! them visible to the Read Recheck and to `shell_poll` and `shell_cancel`.
use crate::{
	Error, Result,
	ports::capabilities::{
		AcceptedOperation, OperationAdmissionScope,
		execution_pod::{
			CommandRequest, CommandResult, ExecutionPodError, ExecutionPodErrorCode,
			PythonStartRequest,
		},
		pod_operations::{PodContext, PodOperation, PodOperationRepository},
	},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		errors::{CapabilityError, TimedOut},
		operations::{POD_EPOCH, POD_RUNTIME, ShellRequest},
	},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;

pub mod files;
pub mod previews;
pub mod processes;
pub mod save;

pub const SHELL: &str = "shell";
pub const PYTHON: &str = "code_interpreter";

/// Admit a command or Python cell for the Execution Pod. The Working Area is
/// neither checked nor held: the command writes the Scratch Space, so
/// `expected_revision` is accepted but not compared (#745).
pub async fn accept<O: Send>(
	scope: &mut dyn OperationAdmissionScope<O>,
	run: &RunMetadata,
	input: ShellRequest,
	kind: &str,
	scratch_space_id: &str,
) -> Result<Value> {
	let limits = scope.limits();
	let seconds = input.seconds(limits)?;
	let extra = json!({"runtime": POD_RUNTIME, "scratch_space_id": scratch_space_id});
	let digest = input.digest(kind, run.id, &extra);
	let key = input.key(run.id);
	if let Some((previous, previous_digest)) = scope.previous(&key).await? {
		if previous_digest != digest {
			return Err(Error::Conflict("IDEMPOTENCY_CONFLICT".into()));
		}
		return scope.result(&previous, 0).await;
	}
	if !limits.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if scope.active_run().await? != Some(run.id) || run.phase.is_terminal() {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	scope.require_write().await?;
	scope.admit_sandboxed_tool().await?;
	let id = scope.operation_id();
	let mut value = json!({
		"command": input.command,
		"seconds": seconds,
		"maximum_seconds": limits.maximum_seconds,
		"revision": scope.area().revision,
	});
	value
		.as_object_mut()
		.expect("object literal")
		.extend(extra.as_object().expect("object literal").clone());
	let operation = scope
		.accept(AcceptedOperation {
			id,
			key,
			digest,
			kind: kind.into(),
			epoch: POD_EPOCH,
			input: value,
		})
		.await?;
	scope.result(&operation, 0).await
}

fn active(state: &str) -> bool {
	matches!(state, "prepared" | "submitted" | "running" | "cancelling")
}

/// The request a Pod operation sends. Built the same way on every retry, so
/// the Runner, which compares the whole body, accepts a repeat as the same
/// command.
pub fn command_request(operation: &PodOperation, context: &PodContext) -> Result<CommandRequest> {
	let seconds = operation.input["seconds"]
		.as_u64()
		.ok_or_else(|| Error::Invalid("Pod operation has no time limit".into()))?;
	Ok(CommandRequest {
		operation_id: operation.id.to_string(),
		scratch_space_id: context.scratch_space_id().to_owned(),
		run_id: operation.run_id.to_string(),
		code: operation.input["command"]
			.as_str()
			.ok_or_else(|| Error::Invalid("Pod operation has no command".into()))?
			.to_owned(),
		cwd: None,
		timeout_seconds: Some(u32::try_from(seconds).unwrap_or(u32::MAX)),
		background: false,
		proxy_credential: context.credential.clone(),
	})
}

fn decode(value: Option<&str>) -> Vec<u8> {
	value
		.and_then(|encoded| STANDARD.decode(encoded).ok())
		.unwrap_or_default()
}

/// A Runner error code as an Agent reads it, such as `PYTHON_UNAVAILABLE`.
fn agent_code(code: &str) -> String {
	code.to_ascii_uppercase()
}

/// A stored error an Agent reads from the operation result.
fn error_value(code: &str, message: &str) -> Value {
	serde_json::to_value(CapabilityError {
		code: code.to_owned(),
		message: message.to_owned(),
		retryable: false,
		details: None,
	})
	.expect("serializable error")
}

/// The state and result a Runner report gives a Pod operation. `now` is
/// seconds since the epoch, used as the start when the Runner first reports
/// the command running; the Runner's `CommandResult` has no start time.
pub fn observe(operation: &PodOperation, reply: &CommandResult, now: f64) -> (String, Value) {
	let mut result = operation.result.clone();
	if !result.is_object() {
		result = json!({});
	}
	let python = operation.kind == PYTHON;
	let status = reply.status.as_str();
	let object = result.as_object_mut().expect("object result");
	object.remove("dispatch_pending");
	// The Runner's journal holds the output; there is no `/v1` receipt to ACK.
	object.insert("runner_acknowledged".into(), json!(true));
	object.insert("pod_status".into(), json!(status));
	if status != "queued" && !object.contains_key("started_at") {
		object.insert("started_at".into(), json!(now));
	}
	if let Some(left) = &reply.left_running {
		object.insert("left_running".into(), json!(left));
	}
	let terminal = !matches!(status, "queued" | "running");
	let mut output = Vec::new();
	let mut error = Value::Null;
	let mut failed = false;
	if python && let Some(cell) = &reply.python_result {
		output.extend_from_slice(cell["stdout"].as_str().unwrap_or_default().as_bytes());
		if cell["truncated"] == true {
			object.insert("truncated".into(), json!(true));
		}
		if cell["status"] == "failed" {
			failed = true;
			error = match cell["code"].as_str() {
				Some(code) => error_value(
					&agent_code(code),
					cell["message"]
						.as_str()
						.unwrap_or("The Python session failed"),
				),
				None => error_value(
					"PYTHON_ERROR",
					"The cell raised an exception; the traceback is in the output.",
				),
			};
		}
	} else if !python {
		output = decode(reply.stdout.as_deref());
		// While the command runs only stdout grows, so offsets stay valid;
		// stderr follows it once the command has ended.
		if terminal {
			let stderr = decode(reply.stderr.as_deref());
			if !stderr.is_empty() {
				if !output.is_empty() && !output.ends_with(b"\n") {
					output.push(b'\n');
				}
				output.extend_from_slice(&stderr);
			}
		}
		if reply.truncated == Some(true) {
			object.insert("truncated".into(), json!(true));
		}
	}
	object.insert("preview_bytes".into(), json!(STANDARD.encode(&output)));
	object.remove("preview");
	if let Some(code) = reply.exit_code {
		object.insert("exit_code".into(), json!(code));
	}
	let state = match status {
		"queued" => "submitted",
		"running" => "running",
		"completed" if failed => "failed",
		"completed" => "completed",
		"timed_out" => {
			let seconds = operation.input["seconds"].as_u64().unwrap_or_default();
			error = serde_json::to_value(CapabilityError::command_timed_out_in_pod(&TimedOut {
				limit_seconds: seconds,
				maximum_seconds: operation.input["maximum_seconds"]
					.as_u64()
					.unwrap_or(seconds),
				revision: operation.input["revision"].as_i64().unwrap_or_default(),
				python,
				install: false,
			}))
			.expect("serializable error");
			object.insert("deadline_expired".into(), json!(true));
			"failed"
		}
		"stopped" | "cancelled" => "cancelled",
		"interrupted" => {
			error = error_value(
				"EXECUTION_UNVERIFIED",
				"The Runner lost track of this command; its effects are unknown. Inspect the Scratch Space before running it again.",
			);
			"uncertain"
		}
		_ => {
			if let Some(code) = &reply.error_code {
				error = error_value(
					&agent_code(code),
					reply.message.as_deref().unwrap_or("The command failed"),
				);
			}
			"failed"
		}
	};
	if terminal {
		// A deadline is shown only while the command runs (#756).
		object.remove("started_at");
		object.insert("termination_confirmed".into(), json!(state != "uncertain"));
		object.insert(
			"effects_may_have_occurred".into(),
			json!(reply.outcome.as_deref() != Some("not_started")),
		);
	}
	if !error.is_null() {
		object.insert("error".into(), error);
	}
	(state.into(), result)
}

/// A refusal the Runner answered before running anything.
fn refused(operation: &PodOperation, code: &str, message: &str) -> Value {
	let mut result = operation.result.clone();
	if !result.is_object() {
		result = json!({});
	}
	let object = result.as_object_mut().expect("object result");
	object.remove("dispatch_pending");
	object.insert("runner_acknowledged".into(), json!(true));
	object.insert("termination_confirmed".into(), json!(true));
	object.insert("effects_may_have_occurred".into(), json!(false));
	object.insert("error".into(), error_value(&agent_code(code), message));
	result
}

fn now() -> f64 {
	chrono::Utc::now().timestamp_millis() as f64 / 1000.0
}

/// An Execution Pod error as the tool layer reports it. A Runner refusal keeps
/// its code, so an Agent reads `INVALID_PATH` or `PYTHON_UNAVAILABLE`.
pub fn tool_error(error: ExecutionPodError) -> Error {
	match error {
		ExecutionPodError::Remote { code, message } => {
			let text = format!("{}: {message}", agent_code(&code.to_string()));
			match code {
				ExecutionPodErrorCode::InvalidPath
				| ExecutionPodErrorCode::InvalidRequest
				| ExecutionPodErrorCode::RequestRejected => Error::Invalid(text),
				ExecutionPodErrorCode::CommandNotFound | ExecutionPodErrorCode::NotBackground => {
					Error::NotFound(text)
				}
				// A file the Agent named is missing, or is not a regular file.
				ExecutionPodErrorCode::Unknown(ref code) if code == "file_error" => {
					Error::Invalid(text)
				}
				_ => Error::Conflict(text),
			}
		}
		ExecutionPodError::Transport(error) => error,
	}
}

/// Start the Pod if needed. Never call this inside an authority transaction:
/// a first start waits for the Pod to become ready.
pub async fn ensure(context: &PodContext) -> std::result::Result<(), ExecutionPodError> {
	context.runtime.ensure(&context.settings).await.map(|_| ())
}

async fn submit(
	repository: &dyn PodOperationRepository,
	operation: &PodOperation,
	context: &PodContext,
) -> Result<()> {
	let request = command_request(operation, context)?;
	// A withdrawal or cancel may have moved the row since it was read; such a
	// row is never sent.
	let current = repository.load(operation.id).await?;
	if current.state != operation.state || current.result["dispatch_pending"] != true {
		return Ok(());
	}
	let reply = if operation.kind == PYTHON {
		// The cell would start a session without the Run credential, so the
		// session is started first with it (ADR 0136).
		match context
			.runtime
			.python_start(&PythonStartRequest {
				scratch_space_id: request.scratch_space_id.clone(),
				run_id: request.run_id.clone(),
				proxy_credential: context.credential.clone(),
			})
			.await
		{
			Ok(_) => context.runtime.python_cell(&request).await,
			Err(error) => Err(error),
		}
	} else {
		context.runtime.execute(&request).await
	};
	match reply {
		Ok(reply) => {
			let (state, result) = observe(operation, &reply, now());
			repository
				.save(operation.id, &operation.state, &state, &result)
				.await?;
			Ok(())
		}
		Err(ExecutionPodError::Remote { code, message }) => {
			// The Runner answered, so it holds nothing for this operation.
			repository
				.save(
					operation.id,
					&operation.state,
					"failed",
					&refused(operation, &code.to_string(), &message),
				)
				.await?;
			Ok(())
		}
		// Unknown outcome: the next pass sends the same request again, which
		// the Runner answers from its journal if it already holds it.
		Err(ExecutionPodError::Transport(error)) => Err(error),
	}
}

/// Advance one Pod operation by one step.
pub async fn drive(repository: &dyn PodOperationRepository, id: Uuid) -> Result<()> {
	let operation = repository.load(id).await?;
	if !active(&operation.state) {
		return Ok(());
	}
	if operation.state == "prepared" && operation.run_finished {
		let mut result = refused(
			&operation,
			"RUN_NOT_ACTIVE",
			"The Run finished before the command started.",
		);
		result["termination_confirmed"] = json!(true);
		repository
			.save(operation.id, "prepared", "cancelled", &result)
			.await?;
		return Ok(());
	}
	let context = match repository.context(&operation).await {
		Ok(context) => context,
		Err(Error::Forbidden | Error::NotFound(_) | Error::Unauthorized)
			if operation.state == "prepared" =>
		{
			repository
				.save(
					operation.id,
					"prepared",
					"failed",
					&refused(
						&operation,
						"SCRATCH_SPACE_UNAVAILABLE",
						"The Run's Scratch Space cannot run commands now.",
					),
				)
				.await?;
			return Ok(());
		}
		Err(error) => return Err(error),
	};
	let dispatch_pending = operation.result["dispatch_pending"] == true;
	match operation.state.as_str() {
		"prepared" => {
			if let Err(error) = ensure(&context).await {
				return match error {
					ExecutionPodError::Remote { code, message } => repository
						.save(
							operation.id,
							"prepared",
							"failed",
							&refused(&operation, &code.to_string(), &message),
						)
						.await
						.map(|_| ()),
					ExecutionPodError::Transport(error) => Err(error),
				};
			}
			// Record the intent to send before sending, so a lost reply is
			// retried with the same request rather than treated as never sent.
			let mut pending = operation.result.clone();
			if !pending.is_object() {
				pending = json!({});
			}
			pending["dispatch_pending"] = json!(true);
			pending["runner_acknowledged"] = json!(true);
			if !repository
				.save(operation.id, "prepared", "submitted", &pending)
				.await?
			{
				return Ok(());
			}
			let submitted = PodOperation {
				state: "submitted".into(),
				result: pending,
				..operation
			};
			submit(repository, &submitted, &context).await
		}
		"submitted" if dispatch_pending && !operation.run_finished => {
			submit(repository, &operation, &context).await
		}
		"cancelling" | "submitted" | "running" => {
			let stopping = operation.state == "cancelling" || operation.run_finished;
			if stopping {
				// The Runner serializes a Run's foreground commands, so stopping
				// the Run's foreground work stops this one and leaves its
				// Background processes and previews running.
				context
					.runtime
					.cancel_waiting(context.scratch_space_id(), &operation.run_id.to_string())
					.await
					.map_err(tool_error)?;
			}
			match context.runtime.result(&operation.id.to_string()).await {
				Ok(reply) => {
					let (mut state, result) = observe(&operation, &reply, now());
					if stopping && active(&state) {
						state = "cancelling".into();
					}
					if state != operation.state || result != operation.result {
						repository
							.save(operation.id, &operation.state, &state, &result)
							.await?;
					}
					Ok(())
				}
				Err(ExecutionPodError::Remote {
					code: ExecutionPodErrorCode::CommandNotFound,
					..
				}) if dispatch_pending => {
					// The Runner does not hold it yet, but a delayed send of the same
					// request could still start it, and `/v2` has no durable refusal
					// for an operation id. So nothing is proven: it ends uncertain,
					// with no ACK claim, rather than as never run.
					let mut result = operation.result.clone();
					if let Some(object) = result.as_object_mut() {
						object.remove("dispatch_pending");
						object.remove("runner_acknowledged");
					}
					result["termination_confirmed"] = json!(false);
					result["effects_may_have_occurred"] = json!(true);
					result["error"] = error_value(
						"EXECUTION_UNVERIFIED",
						"The command may not have reached the Runner, and a delayed send could still start it; its effects are unknown.",
					);
					repository
						.save(operation.id, &operation.state, "uncertain", &result)
						.await
						.map(|_| ())
				}
				Err(ExecutionPodError::Remote {
					code: ExecutionPodErrorCode::CommandNotFound,
					..
				}) => {
					let mut result = operation.result.clone();
					result["termination_confirmed"] = json!(false);
					result["error"] = error_value(
						"EXECUTION_UNVERIFIED",
						"The Runner no longer knows this command; its effects are unknown.",
					);
					repository
						.save(operation.id, &operation.state, "uncertain", &result)
						.await
						.map(|_| ())
				}
				Err(error) => Err(tool_error(error)),
			}
		}
		_ => Ok(()),
	}
}

/// One pass over active Pod operations. A failing operation never holds up
/// another.
pub async fn sweep(repository: &dyn PodOperationRepository) -> Result<()> {
	for id in repository.pending().await? {
		if let Err(error) = Box::pin(drive(repository, id)).await {
			tracing::warn!(%id, %error, "pod operation pending");
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
