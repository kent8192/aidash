use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
/// Code of a command or Python cell stopped at its Time Limit.
pub const COMMAND_TIMED_OUT: &str = "COMMAND_TIMED_OUT";
/// Code of an error stored as plain text that names no code of its own. Such
/// text says what could not be verified, never that a revision conflicted.
pub const EXECUTION_UNVERIFIED: &str = "EXECUTION_UNVERIFIED";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityError {
	pub code: String,
	pub message: String,
	pub retryable: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub details: Option<Value>,
}
fn bounded(value: &str) -> String {
	value.chars().take(2048).collect()
}
const REVISION_PREFIX: &str = "AREA_REVISION_CHANGED: the working area is at revision ";
/// Refusal text for a stale `expected_revision`; it carries the revision the
/// model must send next so it can recover without guessing.
pub fn area_revision_changed(current: i64) -> String {
	format!("{REVISION_PREFIX}{current}; retry with expected_revision {current}")
}
fn current_revision(message: &str) -> Option<i64> {
	let rest = message.strip_prefix(REVISION_PREFIX)?;
	rest.split(';').next()?.parse().ok()
}
/// The code a message opens with, as in `AREA_BUSY: a command is running`.
fn leading_code(message: &str) -> Option<&str> {
	let prefix = message.split([':', ' ']).next().unwrap_or_default();
	(!prefix.is_empty()
		&& prefix
			.bytes()
			.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'))
	.then_some(prefix)
}
/// A command or Python cell that Aidash stopped at its Time Limit, once its
/// file changes are saved.
pub struct TimedOut {
	pub limit_seconds: u64,
	pub maximum_seconds: u64,
	pub revision: i64,
	pub python: bool,
	pub install: bool,
}
/// A refusal's message once the command had also reached its Time Limit.
pub fn also_timed_out(message: &str, limit_seconds: u64) -> String {
	format!(
		"{}. The command had also reached its {limit_seconds} s time limit.",
		message.trim_end_matches('.')
	)
}
impl CapabilityError {
	pub fn message(status: u16, message: &str) -> Self {
		let code = leading_code(message).unwrap_or(match status {
			400 | 413 | 415 | 422 => "INVALID_INPUT",
			401 => "UNAUTHORIZED",
			403 => "FORBIDDEN",
			404 => "NOT_FOUND",
			409 => "CONFLICT",
			429 => "RATE_LIMITED",
			_ => "CAPABILITY_UNAVAILABLE",
		});
		Self {
			code: code.into(),
			message: bounded(message),
			retryable: matches!(status, 429 | 503),
			details: current_revision(message)
				.map(|revision| json!({"current_revision": revision})),
		}
	}
	/// `retryable: false` alone did not stop an Agent repeating a stopped
	/// command, so the message says what to do instead.
	pub fn command_timed_out(stop: &TimedOut) -> Self {
		let &TimedOut {
			limit_seconds,
			maximum_seconds,
			revision,
			python,
			install,
		} = stop;
		let (stopped, lost, pieces) = if python {
			(
				", and the Python session was reset",
				"its variables and anything outside /work, such as /tmp, are gone",
				"shorter cells",
			)
		} else {
			(
				"",
				"anything outside /work, such as /tmp, is gone",
				"shorter commands",
			)
		};
		let longer = if limit_seconds < maximum_seconds {
			format!("raise timeout_seconds (at most {maximum_seconds}), ")
		} else {
			String::new()
		};
		let advice = if install {
			format!(
				"{longer}choose a smaller complete set of wheels, including its dependencies, for the replacement environment."
			)
		} else {
			format!("{longer}split it into {pieces} or print progress.")
		};
		Self {
			code: COMMAND_TIMED_OUT.into(),
			message: format!(
				"Stopped at its {limit_seconds} s time limit{stopped}. File changes in /work were saved as revision {revision}; {lost}. Running it again unchanged will stop the same way: {advice}"
			),
			retryable: false,
			details: Some(json!({"limit_seconds": limit_seconds, "revision": revision})),
		}
	}
	pub fn stored(value: &Value) -> Option<Self> {
		// An empty diagnostic says nothing, so the exit code stands alone.
		if value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty()) {
			return None;
		}
		if let Ok(mut error) = serde_json::from_value::<Self>(value.clone()) {
			error.message = bounded(&error.message);
			return Some(error);
		}
		if let Some(kind) = value["type"].as_str() {
			return Some(Self {
				code: "PYTHON_ERROR".into(),
				message: bounded(value["message"].as_str().unwrap_or("Python failed")),
				retryable: false,
				details: Some(json!({"type":bounded(kind)})),
			});
		}
		if let Some(code) = value["code"].as_str() {
			return Some(Self {
				code: bounded(code),
				message: bounded(value["message"].as_str().unwrap_or("Execution stopped")),
				retryable: false,
				details: None,
			});
		}
		let message = value
			.as_str()
			.unwrap_or("Execution could not be verified; inspect the operation before retrying.");
		let mut error = Self::message(409, message);
		if leading_code(message).is_none() {
			error.code = EXECUTION_UNVERIFIED.into();
		}
		Some(error)
	}
}
#[cfg(test)]
mod tests;
