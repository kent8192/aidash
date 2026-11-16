use super::*;
use crate::Result;
use crate::ports::capabilities::journal_query::{Authority, JournalSessions, Session};
use async_trait::async_trait;
use std::collections::BTreeMap;
use uuid::Uuid;

struct Sessions {
	rows: BTreeMap<Uuid, Session>,
	stopped: Vec<Uuid>,
	confirm: bool,
}

#[async_trait]
impl JournalSessions for Sessions {
	async fn load(&mut self, run: Uuid) -> Result<Option<Session>> {
		Ok(self.rows.get(&run).cloned())
	}
	async fn stop(&mut self, session: Uuid) -> Result<bool> {
		self.stopped.push(session);
		Ok(self.confirm)
	}
	async fn save(&mut self, run: Uuid, session: &Session) -> Result<()> {
		self.rows.insert(run, session.clone());
		Ok(())
	}
}

fn authority(run: Uuid, generation: &str) -> Authority {
	Authority {
		run,
		credential: Uuid::from_u128(1),
		subjects: vec!["alice".into(), "agent".into()],
		policy_revision: 7,
		image: "image@sha256:one".into(),
		runner_instance: "runner-one".into(),
		configuration: "resource-limits".into(),
		generation: generation.into(),
		range: None,
	}
}

#[tokio::test]
async fn narrowing_a_range_does_not_reuse_the_wider_heap() {
	let run = Uuid::new_v4();
	let old = authority(run, "same");
	let old_id = Uuid::new_v4();
	let mut scope = Sessions {
		rows: BTreeMap::from([(
			run,
			Session {
				id: old_id,
				fingerprint: fingerprint(&old),
			},
		)]),
		stopped: vec![],
		confirm: true,
	};
	let mut narrowed = old;
	narrowed.range = Some((4, 7));
	assert!(matches!(
		open_session(&mut scope, &narrowed).await.unwrap(),
		OpenSession::Reset { .. }
	));
	assert_eq!(scope.stopped, vec![old_id]);
}

#[tokio::test]
async fn withdrawal_stops_the_old_heap_before_the_next_cell_can_use_a_fresh_one() {
	let run = Uuid::new_v4();
	let old_id = Uuid::new_v4();
	let old = authority(run, "before");
	let mut scope = Sessions {
		rows: BTreeMap::from([(
			run,
			Session {
				id: old_id,
				fingerprint: fingerprint(&old),
			},
		)]),
		stopped: vec![],
		confirm: false,
	};
	let now = authority(run, "after");
	assert!(open_session(&mut scope, &now).await.is_err());
	assert_eq!(scope.rows[&run].id, old_id);
	scope.confirm = true;
	let OpenSession::Reset { session_id } = open_session(&mut scope, &now).await.unwrap() else {
		panic!("expected reset")
	};
	assert_ne!(session_id, old_id);
	assert_eq!(scope.stopped, vec![old_id, old_id]);
	let OpenSession::Ready {
		session_id: resumed,
	} = open_session(&mut scope, &now).await.unwrap()
	else {
		panic!("expected fresh session")
	};
	assert_eq!(session_id, resumed);
}

#[tokio::test]
async fn another_run_does_not_reuse_the_first_runs_heap() {
	let mut scope = Sessions {
		rows: BTreeMap::new(),
		stopped: vec![],
		confirm: true,
	};
	let one = authority(Uuid::new_v4(), "same");
	let two = authority(Uuid::new_v4(), "same");
	let OpenSession::Ready { session_id: first } = open_session(&mut scope, &one).await.unwrap()
	else {
		panic!()
	};
	let OpenSession::Ready { session_id: second } = open_session(&mut scope, &two).await.unwrap()
	else {
		panic!()
	};
	assert_ne!(first, second);
	assert!(scope.stopped.is_empty());
}
