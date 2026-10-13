//! Completion Policy, Checks and the deterministic Completion Gate (#112).
//!
//! A Run whose Agent definition names a Completion Policy completes its Task
//! only when current Verification Evidence supports its Completion Candidate
//! or no Check applies to it. Evidence binds to the digest of every file
//! mounted in the Run's Working Area, never to the Area revision (every
//! publication increments it) and never to the answer's text. Only Checks the
//! harness invokes produce evidence; a model's claim that something passed
//! never does.
use crate::{
	Error, Result,
	capabilities::operations::{FileScope, MountedFile},
	registry::rules,
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The only Completion Policy version.
pub const CHECKS_V1: &str = "checks/1";
/// The only capability a Check may invoke in this version.
pub const SHELL: &str = "shell";
pub const MAX_CHECKS: usize = 8;
pub const MAX_REPAIR_ALLOWANCE: u32 = 16;
/// A Check's own timeout never exceeds the runner's operation ceiling.
pub const MAX_CHECK_SECONDS: u64 = 600;
pub const MAX_VERIFICATION_SECONDS: u64 = 3600;
pub const MAX_COMMAND_BYTES: usize = 65_536;
/// Diagnostics kept per Check invocation: the tail of its output.
pub const MAX_DIAGNOSTIC_BYTES: usize = 4096;
/// Evidence kept per Run. Older entries can no longer match the content of a
/// later candidate; each invocation stays durable as its capability operation.
const MAX_EVIDENCE: usize = 32;
const DEFAULT_REPAIR_ALLOWANCE: u32 = 3;
const DEFAULT_VERIFICATION_SECONDS: u64 = 1800;

/// What a Completion Candidate must pass before its Run may complete, fixed
/// with the Run's Binding snapshot. An Agent definition that names none
/// requires no verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "version", deny_unknown_fields)]
pub enum CompletionPolicy {
	#[serde(rename = "checks/1")]
	ChecksV1 {
		checks: Vec<Check>,
		/// Repairs allowed per accepted human input before a Completion Review.
		#[serde(default = "repair_allowance")]
		repair_allowance: u32,
		/// Total Check time the whole Run may spend.
		#[serde(default = "verification_seconds")]
		verification_seconds: u64,
		/// Whether a Completion Exception may complete a refused candidate.
		#[serde(default, skip_serializing_if = "std::ops::Not::not")]
		exception: bool,
	},
}

fn repair_allowance() -> u32 {
	DEFAULT_REPAIR_ALLOWANCE
}
fn verification_seconds() -> u64 {
	DEFAULT_VERIFICATION_SECONDS
}

impl CompletionPolicy {
	pub fn checks(&self) -> &[Check] {
		let Self::ChecksV1 { checks, .. } = self;
		checks
	}
	pub fn repair_allowance(&self) -> u32 {
		let Self::ChecksV1 {
			repair_allowance, ..
		} = self;
		*repair_allowance
	}
	pub fn verification_seconds(&self) -> u64 {
		let Self::ChecksV1 {
			verification_seconds,
			..
		} = self;
		*verification_seconds
	}
	pub fn exception_permitted(&self) -> bool {
		let Self::ChecksV1 { exception, .. } = self;
		*exception
	}
	/// Whether any Check depends on the content the Run started from.
	pub fn needs_start_content(&self) -> bool {
		self.checks()
			.iter()
			.any(|check| check.when == CheckWhen::ContentChanged)
	}
	pub fn validate(&self) -> Result<()> {
		let checks = self.checks();
		if checks.is_empty()
			|| checks.len() > MAX_CHECKS
			|| self.repair_allowance() > MAX_REPAIR_ALLOWANCE
			|| !(1..=MAX_VERIFICATION_SECONDS).contains(&self.verification_seconds())
		{
			return Err(Error::Invalid(format!(
				"Completion Policy requires 1 to {MAX_CHECKS} Checks, repair_allowance <= {MAX_REPAIR_ALLOWANCE} and verification_seconds within 1..={MAX_VERIFICATION_SECONDS}"
			)));
		}
		let mut names = std::collections::BTreeSet::new();
		for check in checks {
			check.validate()?;
			if !names.insert(check.name.as_str()) {
				return Err(Error::Invalid(format!(
					"duplicate Check name {}",
					check.name
				)));
			}
		}
		Ok(())
	}
}

/// One entry of a Completion Policy: a bound capability that Aidash invokes
/// with arguments the policy fixes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Check {
	pub name: String,
	/// The bound operation the Check invokes; only `shell` in this version.
	pub capability: String,
	/// The exact command; the model can never change it.
	pub command: String,
	/// Defaults to the node's ordinary operation time.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub timeout_seconds: Option<u64>,
	#[serde(default, skip_serializing_if = "CheckWhen::is_default")]
	pub when: CheckWhen,
}

impl Check {
	pub fn validate(&self) -> Result<()> {
		let name_ok = !self.name.is_empty()
			&& self.name.len() <= 64
			&& self.name.bytes().enumerate().all(|(index, byte)| {
				byte.is_ascii_lowercase()
					|| byte.is_ascii_digit()
					|| index > 0 && matches!(byte, b'_' | b'-')
			});
		if !name_ok
			|| self.capability != SHELL
			|| self.command.trim().is_empty()
			|| self.command.len() > MAX_COMMAND_BYTES
			|| self.command.contains('\0')
			|| self
				.timeout_seconds
				.is_some_and(|seconds| !(1..=MAX_CHECK_SECONDS).contains(&seconds))
		{
			return Err(Error::Invalid(format!(
				"Check {:?} requires a lowercase name, capability \"shell\", a command of at most {MAX_COMMAND_BYTES} bytes and timeout_seconds within 1..={MAX_CHECK_SECONDS}",
				self.name
			)));
		}
		Ok(())
	}
	/// Identity of what the Check runs. Evidence of another definition never counts.
	pub fn digest(&self) -> String {
		rules::digest(&json!(self))
	}
}

/// When a Check applies to a Completion Candidate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckWhen {
	/// To every candidate.
	Always,
	/// Only when the mounted content differs from the content the Run started with.
	#[default]
	ContentChanged,
}
impl CheckWhen {
	fn is_default(&self) -> bool {
		*self == Self::default()
	}
}

/// Identifies the exact files mounted in a Working Area, across every scope.
pub fn content_digest(files: &[MountedFile]) -> String {
	let mut entries = files
		.iter()
		.map(|file| {
			let scope = match file.scope {
				FileScope::Working => "working",
				FileScope::References => "references",
				FileScope::Received => "received",
			};
			(scope, file.path.as_str(), file.digest.as_str())
		})
		.collect::<Vec<_>>();
	entries.sort_unstable();
	rules::digest(&json!(entries))
}

/// The content digest of a Run without a Working Area: nothing is mounted.
pub fn empty_content() -> String {
	content_digest(&[])
}

/// What one Check invocation tested. Evidence counts only for an identical subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckSubject {
	pub check: String,
	/// [`Check::digest`] of the definition that ran.
	pub check_digest: String,
	/// Digest of the bound capability definition that ran it.
	pub capability_digest: String,
	/// [`content_digest`] of the files the Check saw.
	pub content_digest: String,
}

impl CheckSubject {
	/// The idempotency key of one attempt. Re-deriving it after a process cut
	/// returns the operation an earlier attempt already started.
	pub fn key(&self, run: Uuid, attempt: u32) -> Uuid {
		let mut hasher = Sha256::new();
		hasher.update(run.as_bytes());
		hasher.update(json!([self, attempt]).to_string().as_bytes());
		let digest = hasher.finalize();
		let mut bytes = [0; 16];
		bytes.copy_from_slice(&digest[..16]);
		uuid::Builder::from_custom_bytes(bytes).into_uuid()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
	Passed,
	/// The Check ran and did not pass.
	Failed,
	/// The Check ran past its timeout.
	TimedOut,
	/// The Check could not run, or its result is unknown.
	Unavailable,
	/// Authority or an approval refused the Check.
	Denied,
}
impl CheckOutcome {
	/// The model can repair the candidate; other refusals need a person.
	pub fn repairable(self) -> bool {
		matches!(self, Self::Failed | Self::TimedOut)
	}
}

/// The record of one Check's actual invocation against exact content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationEvidence {
	pub subject: CheckSubject,
	/// The capability operation that ran; absent when the Check never started.
	pub operation_id: Option<Uuid>,
	pub attempt: u32,
	pub outcome: CheckOutcome,
	pub exit_code: Option<i64>,
	/// Bounded tail of the Check's output or the reason it could not run.
	pub diagnostics: String,
	pub seconds: u64,
	pub finished_at: DateTime<Utc>,
}

/// A started Check whose operation has not been recorded as evidence yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PendingCheck {
	pub subject: CheckSubject,
	pub operation_id: Uuid,
	pub attempt: u32,
	pub timeout_seconds: u64,
	pub started_at: DateTime<Utc>,
}

impl PendingCheck {
	/// Record the finished operation. A Check that ran for its whole timeout
	/// without passing timed out.
	pub fn finish(
		self,
		passed: bool,
		exit_code: Option<i64>,
		diagnostics: &str,
		now: DateTime<Utc>,
	) -> VerificationEvidence {
		let seconds = elapsed(self.started_at, now);
		let outcome = if passed {
			CheckOutcome::Passed
		} else if seconds >= self.timeout_seconds {
			CheckOutcome::TimedOut
		} else {
			CheckOutcome::Failed
		};
		self.evidence(outcome, exit_code, diagnostics, now)
	}
	/// Record an operation whose result cannot be used.
	pub fn unavailable(self, detail: &str, now: DateTime<Utc>) -> VerificationEvidence {
		self.evidence(CheckOutcome::Unavailable, None, detail, now)
	}
	fn evidence(
		self,
		outcome: CheckOutcome,
		exit_code: Option<i64>,
		diagnostics: &str,
		now: DateTime<Utc>,
	) -> VerificationEvidence {
		VerificationEvidence {
			outcome,
			exit_code,
			diagnostics: bounded_tail(diagnostics, MAX_DIAGNOSTIC_BYTES),
			seconds: elapsed(self.started_at, now),
			finished_at: now,
			operation_id: Some(self.operation_id),
			attempt: self.attempt,
			subject: self.subject,
		}
	}
	/// Whether the operation is overdue even allowing the runner's own grace.
	pub fn overdue(&self, now: DateTime<Utc>) -> bool {
		elapsed(self.started_at, now) > self.timeout_seconds.saturating_add(CHECK_GRACE_SECONDS)
	}
}

impl CheckSubject {
	/// Evidence of a Check that never started.
	pub fn refused(
		self,
		attempt: u32,
		outcome: CheckOutcome,
		detail: &str,
		now: DateTime<Utc>,
	) -> VerificationEvidence {
		VerificationEvidence {
			subject: self,
			operation_id: None,
			attempt,
			outcome,
			exit_code: None,
			diagnostics: bounded_tail(detail, MAX_DIAGNOSTIC_BYTES),
			seconds: 0,
			finished_at: now,
		}
	}
}

/// Time the runner may take beyond a Check's timeout to confirm it stopped.
pub const CHECK_GRACE_SECONDS: u64 = 120;

fn elapsed(from: DateTime<Utc>, to: DateTime<Utc>) -> u64 {
	u64::try_from((to - from).num_seconds()).unwrap_or(0)
}

/// The end of `text`, at most `limit` bytes, cut at a character boundary.
pub fn bounded_tail(text: &str, limit: usize) -> String {
	if text.len() <= limit {
		return text.to_owned();
	}
	let mut start = text.len() - limit;
	while !text.is_char_boundary(start) {
		start += 1;
	}
	text[start..].to_owned()
}

/// Run-lifetime Completion Gate state, kept with the Run's context and never
/// sent to the model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletionProgress {
	/// Content digest when the Run first executed; `None` until recorded.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub start_content: Option<String>,
	/// The accepted human input the Repair Allowance belongs to.
	#[serde(default)]
	pub input_seq: i64,
	#[serde(default)]
	pub repairs: u32,
	/// Check attempts started by this Run; numbers each attempt's key.
	#[serde(default)]
	pub attempts: u32,
	/// Check time already spent by this Run.
	#[serde(default)]
	pub seconds: u64,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub evidence: Vec<VerificationEvidence>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub pending: Option<PendingCheck>,
	/// Completion Reviews raised by this Run; numbers each review's request key.
	#[serde(default)]
	pub reviews: u32,
	/// The human request of the Completion Review the Run waits on.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub review: Option<Uuid>,
	/// How the Run's Task was completed, once it was.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub basis: Option<CompletionBasis>,
}

impl CompletionProgress {
	pub fn is_empty(&self) -> bool {
		*self == Self::default()
	}
	/// Each accepted human input renews the Repair Allowance.
	pub fn renew(&mut self, input_seq: i64) {
		if input_seq > self.input_seq {
			self.input_seq = input_seq;
			self.repairs = 0;
		}
	}
	/// Replace earlier evidence of the same subject and keep the newest entries.
	pub fn record(&mut self, evidence: VerificationEvidence) {
		self.seconds = self.seconds.saturating_add(evidence.seconds);
		self.evidence.retain(|old| old.subject != evidence.subject);
		self.evidence.push(evidence);
		if self.evidence.len() > MAX_EVIDENCE {
			let excess = self.evidence.len() - MAX_EVIDENCE;
			self.evidence.drain(..excess);
		}
	}
	pub fn latest(&self, subject: &CheckSubject) -> Option<&VerificationEvidence> {
		self.evidence
			.iter()
			.rev()
			.find(|evidence| &evidence.subject == subject)
	}
	/// A person asked for the Checks to run again: only passing evidence stays.
	/// The Repair Allowance and the Run's verification time are unchanged.
	pub fn retry(&mut self) {
		self.evidence
			.retain(|evidence| evidence.outcome == CheckOutcome::Passed);
	}
}

/// How a Task came to be completed. Only `Verified` means Checks passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompletionBasis {
	Verified,
	NotRequired,
	Excepted,
}
impl CompletionBasis {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Verified => "verified",
			Self::NotRequired => "not_required",
			Self::Excepted => "excepted",
		}
	}
	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"verified" => Ok(Self::Verified),
			"not_required" => Ok(Self::NotRequired),
			"excepted" => Ok(Self::Excepted),
			_ => Err(Error::Invalid(format!("unknown Completion Basis {value}"))),
		}
	}
}

/// Why the Completion Gate pauses the Run for a Completion Review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewReason {
	Unavailable { check: String, detail: String },
	Denied { check: String, detail: String },
	RepairExhausted { check: String },
	TimeExhausted,
}

impl ReviewReason {
	/// The human request text. Diagnostics stay with the evidence.
	pub fn prompt(&self) -> String {
		let reason = match self {
			Self::Unavailable { check, .. } => format!("Check {check} could not run"),
			Self::Denied { check, .. } => format!("Check {check} was refused"),
			Self::RepairExhausted { check } => {
				format!("Check {check} still fails and the Repair Allowance is spent")
			}
			Self::TimeExhausted => "the Run's verification time is spent".into(),
		};
		format!(
			"Completion Review: {reason}. Answer retry to run the Checks again, send a new message to correct the task, or cancel the Run."
		)
	}
}

/// One refused Check, as the model sees it in a repair request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepairNote {
	pub check: String,
	pub outcome: CheckOutcome,
	pub exit_code: Option<i64>,
	pub diagnostics: String,
}
impl From<&VerificationEvidence> for RepairNote {
	fn from(evidence: &VerificationEvidence) -> Self {
		Self {
			check: evidence.subject.check.clone(),
			outcome: evidence.outcome,
			exit_code: evidence.exit_code,
			diagnostics: evidence.diagnostics.clone(),
		}
	}
}

pub const REPAIR_INSTRUCTION: &str = "Your final answer was not accepted: a completion Check did not pass on the current files. Fix the problem, then give your final answer again. The Check runs again on the files as they are then.";

/// The facts the Completion Gate decides on, read when the candidate forms.
pub struct Candidate<'a> {
	/// Digest of the mounted content; `None` when the Run has no Working Area.
	pub content: Option<&'a str>,
	/// Digest of the bound capability Checks invoke; `None` when it is unbound.
	pub capability: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision<'a> {
	Complete(CompletionBasis),
	Start {
		check: &'a Check,
		subject: CheckSubject,
	},
	Repair(VerificationEvidence),
	Review(ReviewReason),
}

/// The deterministic Completion Gate. Checks apply in policy order; the first
/// one without passing evidence decides what happens next.
pub fn decide<'a>(
	policy: &'a CompletionPolicy,
	progress: &CompletionProgress,
	candidate: &Candidate<'_>,
) -> Decision<'a> {
	let empty = empty_content();
	let current = candidate.content.unwrap_or(&empty);
	let mut applied = false;
	for check in policy.checks() {
		let applies = match check.when {
			CheckWhen::Always => true,
			CheckWhen::ContentChanged => progress
				.start_content
				.as_deref()
				.is_none_or(|start| start != current),
		};
		if !applies {
			continue;
		}
		applied = true;
		let (Some(content), Some(capability)) = (candidate.content, candidate.capability) else {
			return Decision::Review(ReviewReason::Unavailable {
				check: check.name.clone(),
				detail: if candidate.content.is_none() {
					"the Run has no Working Area".into()
				} else {
					format!("capability {} is not bound", check.capability)
				},
			});
		};
		let subject = CheckSubject {
			check: check.name.clone(),
			check_digest: check.digest(),
			capability_digest: capability.into(),
			content_digest: content.into(),
		};
		match progress.latest(&subject) {
			None if progress.seconds >= policy.verification_seconds() => {
				return Decision::Review(ReviewReason::TimeExhausted);
			}
			None => return Decision::Start { check, subject },
			Some(evidence) => match evidence.outcome {
				CheckOutcome::Passed => {}
				CheckOutcome::Failed | CheckOutcome::TimedOut
					if progress.repairs < policy.repair_allowance() =>
				{
					return Decision::Repair(evidence.clone());
				}
				CheckOutcome::Failed | CheckOutcome::TimedOut => {
					return Decision::Review(ReviewReason::RepairExhausted {
						check: check.name.clone(),
					});
				}
				CheckOutcome::Unavailable => {
					return Decision::Review(ReviewReason::Unavailable {
						check: check.name.clone(),
						detail: evidence.diagnostics.clone(),
					});
				}
				CheckOutcome::Denied => {
					return Decision::Review(ReviewReason::Denied {
						check: check.name.clone(),
						detail: evidence.diagnostics.clone(),
					});
				}
			},
		}
	}
	Decision::Complete(if applied {
		CompletionBasis::Verified
	} else {
		CompletionBasis::NotRequired
	})
}

#[cfg(test)]
mod tests;
