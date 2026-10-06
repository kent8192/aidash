//! Pure admission and delivery rules for remote run inputs.
use crate::apps::workspaces::models::states::TaskStatus;

pub(crate) fn terminal(status: &TaskStatus) -> bool {
	matches!(
		status,
		TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Abandoned
	)
}
