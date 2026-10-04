use super::*;
use async_trait::async_trait;
use rstest::rstest;
use serde_json::json;
struct Scope {
	live: bool,
	denied: bool,
	grant: Option<Uuid>,
	calls: Vec<&'static str>,
}
fn snapshot() -> WorkspaceSnapshot {
	serde_json::from_value(json!({"workspace":{"id":Uuid::from_u128(2),"title":"workspace","goal":"intent","state":{},"revision":3,"created_at":"2026-10-04T00:00:00Z"},"tasks":[],"artifacts":[],"events":[],"messages":[]})).unwrap()
}
#[async_trait]
impl SnapshotScope for Scope {
	fn select_read_grant(&mut self, grant: Uuid) {
		self.grant = Some(grant);
		self.calls.push("read_grant");
	}
	async fn workspace_snapshot(&mut self, workspace: Uuid) -> Result<WorkspaceSnapshot> {
		assert_eq!(workspace, snapshot().workspace.id);
		assert_eq!(self.grant, Some(Uuid::from_u128(1)));
		self.calls.push("snapshot");
		if self.denied {
			Err(Error::Forbidden)
		} else {
			Ok(snapshot())
		}
	}
	async fn snapshot_grant_live(&mut self, grant: Uuid) -> Result<bool> {
		assert_eq!(self.grant, Some(grant));
		self.calls.push("live");
		Ok(self.live)
	}
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn complete_snapshot_read_requires_same_grant_to_remain_live_at_delivery(#[case] live: bool) {
	let mut scope = Scope {
		live,
		denied: false,
		grant: None,
		calls: vec![],
	};
	let result = read(&mut scope, Uuid::from_u128(1), Uuid::from_u128(2)).await;
	if live {
		assert_eq!(
			serde_json::to_value(result.unwrap()).unwrap(),
			serde_json::to_value(snapshot()).unwrap()
		);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_eq!(scope.calls, vec!["read_grant", "snapshot", "live"]);
}
#[rstest]
#[tokio::test]
async fn snapshot_authority_denial_stops_before_any_delivery_check() {
	let mut scope = Scope {
		live: true,
		denied: true,
		grant: None,
		calls: vec![],
	};
	assert!(matches!(
		read(&mut scope, Uuid::from_u128(1), Uuid::from_u128(2)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["read_grant", "snapshot"]);
}
