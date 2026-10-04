pub use crate::domain::HumanRequest;

use crate::apps::execution::models::{
	HumanRequest as HumanRequestRecord, states::HumanRequestKind,
};
impl From<HumanRequestRecord> for HumanRequest {
	fn from(record: HumanRequestRecord) -> Self {
		Self {
			run_id: record.run_id(),
			answered_by: record.answered_by,
			id: record.id,
			workspace_id: record.workspace_id,
			kind: match record.kind {
				HumanRequestKind::Question => "QUESTION",
				HumanRequestKind::ApprovalRequired => "APPROVAL_REQUIRED",
				HumanRequestKind::Confirmation => "CONFIRMATION",
				HumanRequestKind::InformationRequest => "INFORMATION_REQUEST",
			}
			.into(),
			prompt: record.prompt,
			response: record.response.map(|value| value.0),
			created_at: record.created_at,
		}
	}
}
