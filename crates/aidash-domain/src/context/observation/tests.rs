use super::*;
use crate::{Artifact, Event, Message, Task, Workspace};
use chrono::Utc;
use uuid::Uuid;

fn snapshot(rows: usize) -> WorkspaceSnapshot {
	let workspace_id = Uuid::new_v4();
	let now = Utc::now();
	WorkspaceSnapshot {
		workspace: Workspace {
			id: workspace_id,
			title: "\0".repeat(256),
			goal: "\0".repeat(512),
			state: json!({}),
			revision: 1,
			created_at: now,
		},
		tasks: (0..rows)
			.map(|_| Task {
				id: Uuid::new_v4(),
				workspace_id,
				title: "\0".repeat(256),
				description: "\0".repeat(512),
				status: crate::TaskStatus::Running,
				requirements: json!({}),
				owner: Some("\0".repeat(256)),
				created_by: "subject".into(),
				dependencies: (0..MAX_LIMIT).map(|_| Uuid::new_v4()).collect(),
				parent_id: None,
				revision: 1,
				created_at: now,
			})
			.collect(),
		artifacts: (0..rows)
			.map(|_| Artifact {
				id: Uuid::new_v4(),
				workspace_id,
				task_id: Uuid::new_v4(),
				kind: "text".into(),
				name: "\0".repeat(256),
				content: json!("omitted"),
				created_by: "subject".into(),
				idempotency_key: "artifact".into(),
				created_at: now,
			})
			.collect(),
		events: (0..rows)
			.map(|sequence| Event {
				sequence: sequence as i64,
				id: Uuid::new_v4(),
				node_id: "node".into(),
				workspace_id: Some(workspace_id),
				kind: "\0".repeat(128),
				data: json!({}),
				created_at: now,
			})
			.collect(),
		messages: (0..rows)
			.map(|_| Message {
				id: Uuid::new_v4(),
				workspace_id,
				sender: "\0".repeat(256),
				content: "\0".repeat(512),
				idempotency_key: None,
				created_at: now,
			})
			.collect(),
	}
}

#[rstest::rstest]
fn fit_projection_picks_largest_page_from_one_snapshot() {
	let snapshot = snapshot(3);
	let one = project(&snapshot, 0, 1).to_string().len();
	let two = project(&snapshot, 0, 2).to_string().len();
	let budget = one + (two - one) / 2;
	let fitted = fit_projection(&snapshot, 0, 3, |_, output| {
		Ok::<bool, crate::Error>(output.to_string().len() <= budget)
	})
	.unwrap()
	.unwrap();
	assert_eq!(fitted.0, 1);
	assert!(fitted.1.to_string().len() <= budget);
	assert_eq!(fitted.1["tasks"].as_array().unwrap().len(), 1);
}

#[rstest::rstest]
fn fit_projection_defers_when_even_one_row_cannot_fit() {
	let snapshot = snapshot(1);
	assert!(
		fit_projection(&snapshot, 0, 1, |_, _| Ok::<bool, crate::Error>(false))
			.unwrap()
			.is_none()
	);
}
