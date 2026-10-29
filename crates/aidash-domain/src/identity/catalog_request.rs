//! A person's request that an approver review an exact definition on one Node.
//! It grants nothing; it only records who asked, why and which closure they saw.
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_REASON: usize = 2000;
pub const CATALOG_REQUEST_CREATE: &str = "catalog_request.create";
pub const CATALOG_REQUEST_WITHDRAW: &str = "catalog_request.withdraw";
pub const CATALOG_REQUEST_DECLINE: &str = "catalog_request.decline";
pub const CATALOG_REQUEST_FULFIL: &str = "catalog_request.fulfil";

/// The lifecycle of one Catalog Approval Request. Only `Open` may move, and
/// every other state is final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogApprovalRequestState {
	Open,
	Fulfilled,
	Declined,
	Stale,
	Withdrawn,
}

impl CatalogApprovalRequestState {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Open => "open",
			Self::Fulfilled => "fulfilled",
			Self::Declined => "declined",
			Self::Stale => "stale",
			Self::Withdrawn => "withdrawn",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"open" => Ok(Self::Open),
			"fulfilled" => Ok(Self::Fulfilled),
			"declined" => Ok(Self::Declined),
			"stale" => Ok(Self::Stale),
			"withdrawn" => Ok(Self::Withdrawn),
			_ => Err(Error::UnsupportedState(format!(
				"unknown catalog approval request state: {value}"
			))),
		}
	}

	pub fn is_open(self) -> bool {
		self == Self::Open
	}

	pub fn is_final(self) -> bool {
		!self.is_open()
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CatalogApprovalRequest {
	pub id: Uuid,
	pub tenant: String,
	pub node: crate::nodes::NodeName,
	pub root: crate::registry::EntityRef,
	/// The Tenant subject that asked. It may be a person, service or agent.
	pub requester: String,
	pub reason: String,
	/// The exact closure digest the requester reviewed.
	pub closure_digest: String,
	pub state: CatalogApprovalRequestState,
	pub decline_reason: Option<String>,
	/// The subject that fulfilled, declined or withdrew it. Staleness is
	/// recorded without a deciding subject.
	pub decided_by: Option<String>,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub revision: i64,
}

/// A bounded free-text reason: no blank request and no unbounded record.
pub fn validate_reason(reason: &str) -> Result<()> {
	let reason = reason.trim();
	if reason.is_empty() || reason.chars().count() > MAX_REASON {
		return Err(Error::Invalid(format!(
			"a request reason needs 1..{MAX_REASON} characters"
		)));
	}
	if reason.chars().any(char::is_control) {
		return Err(Error::Invalid(
			"a request reason cannot contain control characters".into(),
		));
	}
	Ok(())
}

/// The fields a new open request is made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCatalogApprovalRequest {
	pub id: Uuid,
	pub tenant: String,
	pub node: crate::nodes::NodeName,
	pub root: crate::registry::EntityRef,
	pub requester: String,
	pub reason: String,
	pub closure_digest: String,
	pub now: DateTime<Utc>,
}

impl CatalogApprovalRequest {
	pub fn new(input: NewCatalogApprovalRequest) -> Result<Self> {
		validate_reason(&input.reason)?;
		if input.closure_digest.trim().is_empty() {
			return Err(Error::Invalid("a request needs a closure digest".into()));
		}
		Ok(Self {
			id: input.id,
			tenant: input.tenant,
			node: input.node,
			root: input.root,
			requester: input.requester,
			reason: input.reason.trim().to_owned(),
			closure_digest: input.closure_digest,
			state: CatalogApprovalRequestState::Open,
			decline_reason: None,
			decided_by: None,
			created_at: input.now,
			updated_at: input.now,
			revision: 1,
		})
	}

	/// Move an open request to one final state. The caller decides who may take
	/// the transition; this rule only keeps the state machine and reason sound.
	pub fn transition(
		&mut self,
		next: CatalogApprovalRequestState,
		decided_by: Option<&str>,
		decline_reason: Option<&str>,
		now: DateTime<Utc>,
	) -> Result<()> {
		if !self.state.is_open() {
			return Err(Error::Conflict(
				"catalog approval request is already final".into(),
			));
		}
		if next == CatalogApprovalRequestState::Open {
			return Err(Error::Invalid(
				"catalog approval request is already open".into(),
			));
		}
		if next == CatalogApprovalRequestState::Declined {
			let reason = decline_reason
				.ok_or_else(|| Error::Invalid("declining a request needs a reason".into()))?;
			validate_reason(reason)?;
			self.decline_reason = Some(reason.trim().to_owned());
		} else if decline_reason.is_some() {
			return Err(Error::Invalid(
				"only a declined request carries a decline reason".into(),
			));
		}
		self.state = next;
		self.decided_by = decided_by.map(str::to_owned);
		self.updated_at = now;
		self.revision += 1;
		Ok(())
	}

	pub fn withdraw(&mut self, by: &str, now: DateTime<Utc>) -> Result<()> {
		self.transition(CatalogApprovalRequestState::Withdrawn, Some(by), None, now)
	}

	pub fn decline(&mut self, by: &str, reason: &str, now: DateTime<Utc>) -> Result<()> {
		self.transition(
			CatalogApprovalRequestState::Declined,
			Some(by),
			Some(reason),
			now,
		)
	}

	pub fn fulfil(&mut self, by: &str, now: DateTime<Utc>) -> Result<()> {
		self.transition(CatalogApprovalRequestState::Fulfilled, Some(by), None, now)
	}

	pub fn stale(&mut self, now: DateTime<Utc>) -> Result<()> {
		self.transition(CatalogApprovalRequestState::Stale, None, None, now)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn request() -> CatalogApprovalRequest {
		CatalogApprovalRequest::new(NewCatalogApprovalRequest {
			id: Uuid::nil(),
			tenant: "acme".into(),
			node: crate::nodes::NodeName::derived("acme"),
			root: crate::registry::EntityRef {
				id: "writer".into(),
				version: "1.0.0".into(),
			},
			requester: "alice".into(),
			reason: "Needs the writer".into(),
			closure_digest: "sha256:closure".into(),
			now: Utc::now(),
		})
		.unwrap()
	}

	#[test]
	fn an_open_request_moves_to_each_final_state_once() {
		for next in [
			CatalogApprovalRequestState::Fulfilled,
			CatalogApprovalRequestState::Declined,
			CatalogApprovalRequestState::Stale,
			CatalogApprovalRequestState::Withdrawn,
		] {
			let mut request = request();
			let reason = (next == CatalogApprovalRequestState::Declined).then_some("Not now");
			request
				.transition(next, Some("bob"), reason, Utc::now())
				.unwrap();
			assert_eq!(request.state, next);
			assert_eq!(request.revision, 2);
			assert!(request.state.is_final());
		}
	}

	#[test]
	fn a_final_request_never_moves_again() {
		let mut request = request();
		request.withdraw("alice", Utc::now()).unwrap();
		for next in [
			CatalogApprovalRequestState::Fulfilled,
			CatalogApprovalRequestState::Declined,
			CatalogApprovalRequestState::Stale,
			CatalogApprovalRequestState::Withdrawn,
		] {
			let reason = (next == CatalogApprovalRequestState::Declined).then_some("No");
			assert!(
				matches!(
					request.transition(next, Some("bob"), reason, Utc::now()),
					Err(Error::Conflict(_))
				),
				"{next:?}"
			);
		}
	}

	#[test]
	fn declining_needs_a_bounded_reason() {
		let mut request = request();
		assert!(matches!(
			request.decline("bob", "   ", Utc::now()),
			Err(Error::Invalid(_))
		));
		assert!(matches!(
			request.decline("bob", &"x".repeat(MAX_REASON + 1), Utc::now()),
			Err(Error::Invalid(_))
		));
		request.decline("bob", "Not needed", Utc::now()).unwrap();
		assert_eq!(request.decline_reason.as_deref(), Some("Not needed"));
	}

	#[test]
	fn a_new_request_starts_open_and_trims_its_reason() {
		let request = request();
		assert_eq!(request.state, CatalogApprovalRequestState::Open);
		assert_eq!(request.reason, "Needs the writer");
		assert_eq!(request.revision, 1);
		assert!(request.decided_by.is_none());
	}
}
