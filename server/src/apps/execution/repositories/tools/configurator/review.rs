use super::Api;
use crate::Result;
use aidash_application::authorization::catalog::review::{
	ApprovalReview, ApprovalReviewSubmission, ReviewActivation, ReviewApproval, ReviewItem,
	ReviewStatus,
};
use aidash_domain::{identity::catalog::ApprovalScope, nodes::NodeName, registry::EntityRef};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Deserialize)]
pub(super) struct Input {
	node: NodeName,
	entry: EntityRef,
	scope: Option<ApprovalScope>,
	root_groups: Option<BTreeSet<String>>,
}

fn selection(item: &ReviewItem, requested: Option<ApprovalScope>) -> Option<ApprovalScope> {
	let available = item.available_scope?;
	let scope = if item.kind == "model" {
		ApprovalScope::Deployment
	} else {
		requested.unwrap_or(available)
	};
	if item.can_approve
		&& scope <= available
		&& (item.awaits_approver()
			|| (scope == ApprovalScope::Deployment
				&& item.installation.is_none()
				&& (item.scope == Some(ApprovalScope::Node) || !item.unapproved_nodes.is_empty())))
	{
		Some(scope)
	} else {
		None
	}
}

fn submission(review: &ApprovalReview, input: &Input) -> ApprovalReviewSubmission {
	let mut sub = ApprovalReviewSubmission {
		node: review.node.clone(),
		root: review.root.clone(),
		closure_digest: review.closure_digest.clone(),
		root_groups: input.root_groups.clone(),
		request: None,
		items: vec![],
		installations: vec![],
	};
	for item in &review.items {
		let Some(scope) = selection(item, input.scope) else {
			continue;
		};
		if let Some(installation) = &item.installation {
			sub.installations.push(ReviewActivation {
				installation: installation.installation.clone(),
				revision: installation.revision,
				digest: installation.digest.clone(),
				expected_activation_revision: installation.expected_activation_revision,
				expected_node_revisions: item.expected_node_revisions.clone(),
				scope: Some(scope),
				carry_over: item.carry_over,
				restriction: item.proposed_restriction.clone(),
				provider_warning_acknowledged: item.provider_warning.is_some(),
			});
		} else if let Some(digest) = &item.digest {
			sub.items.push(ReviewApproval {
				reference: item.reference.clone(),
				digest: digest.clone(),
				expected_revision: item.expected_revision,
				expected_node_revisions: item.expected_node_revisions.clone(),
				scope: Some(scope),
				carry_over: item.carry_over,
				restriction: item.proposed_restriction.clone(),
				provider_warning_acknowledged: item.provider_warning.is_some(),
			});
		}
	}
	sub
}

impl Api<'_> {
	pub(super) async fn review_approve(&self, input: Input) -> Result<Value> {
		let path = format!("/api/authorization/{}/catalog/review", self.caller.tenant());
		use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
		let reference = format!("{}@{}", input.entry.id, input.entry.version);
		let review_path = format!(
			"{path}?node={}&root={}",
			utf8_percent_encode(input.node.as_str(), NON_ALPHANUMERIC),
			utf8_percent_encode(&reference, NON_ALPHANUMERIC)
		);
		let mut review: ApprovalReview =
			serde_json::from_value(self.call("GET", &review_path, Value::Null).await?)?;
		let sub = submission(&review, &input);
		let approved = if sub.items.is_empty() && sub.installations.is_empty() {
			json!({"bindings":[],"installations":[]})
		} else {
			let written = self.call("POST", &path, serde_json::to_value(sub)?).await?;
			review = serde_json::from_value(self.call("GET", &review_path, Value::Null).await?)?;
			written
		};
		let blocked: Vec<_> = review
			.items
			.iter()
			.filter_map(|item| {
				let broad = input.scope == Some(ApprovalScope::Deployment) || item.kind == "model";
				let reason = match item.status {
					ReviewStatus::Missing => "not_registered_or_installed",
					ReviewStatus::ProviderUnavailable => "provider_unavailable",
					ReviewStatus::SecretOrConnectionMissing => "secret_or_connection_missing",
					ReviewStatus::NeedsOtherApprover => "needs_other_approver",
					_ if broad && item.available_scope != Some(ApprovalScope::Deployment) => {
						"deployment_scope_unavailable"
					}
					ReviewStatus::Approved if !item.widen => return None,
					_ if !item.can_approve => "needs_other_approver",
					_ if item.available_scope.is_none() => "dependency_blocked",
					_ => "approval_pending",
				};
				Some(
					json!({"entry":item.reference,"kind":item.kind,"status":item.status,"reason":reason}),
				)
			})
			.collect();
		Ok(
			json!({"approved":approved,"blocked":blocked,"hidden":review.hidden,"ready":review.ready && blocked.is_empty()}),
		)
	}
}
