use aidash_application::{
	Error, Result,
	authorization::{Authorization, Snapshot},
	ports::{AuthorizationScope, AuthorizationStore, AuthorizationTransaction},
};
use aidash_domain::policy::{Decision, Evaluation, PolicyBundle};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
	begun: usize,
	committed: usize,
	rolled_back: usize,
	decisions: Vec<Decision>,
}

struct Store {
	state: Arc<Mutex<State>>,
	fail_audit: bool,
}

struct Transaction {
	state: Arc<Mutex<State>>,
	decisions: Vec<Decision>,
	fail_audit: bool,
	committed: bool,
}

fn bundle() -> PolicyBundle {
	serde_json::from_value(json!({
		"tenant": "acme",
		"subjects": {"worker": {"kind": "agent"}},
		"policies": [{"id":"read", "effect":"allow", "subjects":{"any":true},
			"actions":["workspace.read"], "resources":{"kinds":["workspace"]}}]
	}))
	.unwrap()
}

fn evaluation() -> Evaluation {
	serde_json::from_value(json!({
		"subject":"worker", "action":"workspace.read",
		"resource":{"tenant":"acme", "kind":"workspace", "id":"workspace-1"},
		"environment":{}
	}))
	.unwrap()
}

#[async_trait]
impl AuthorizationStore for Store {
	async fn begin(&self) -> Result<Box<dyn AuthorizationTransaction>> {
		self.state.lock().unwrap().begun += 1;
		Ok(Box::new(Transaction {
			state: self.state.clone(),
			decisions: Vec::new(),
			fail_audit: self.fail_audit,
			committed: false,
		}))
	}
}

#[async_trait]
impl AuthorizationScope for Transaction {
	async fn load(&mut self, tenant: &str) -> Result<Snapshot> {
		assert_eq!(tenant, "acme");
		Ok(Snapshot {
			revision: 7,
			bundle: bundle(),
		})
	}

	async fn record_decision(
		&mut self,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		assert_eq!(tenant, "acme");
		assert_eq!(input.subject, "worker");
		if self.fail_audit {
			return Err(Error::External("audit storage unavailable".into()));
		}
		self.decisions.push(decision.clone());
		Ok(())
	}
}

#[async_trait]
impl AuthorizationTransaction for Transaction {
	async fn replace(&mut self, _: &str, _: i64, _: &PolicyBundle, _: &str) -> Result<i64> {
		Err(Error::Conflict(
			"replacement is not expected in this fixture".into(),
		))
	}

	async fn commit(mut self: Box<Self>) -> Result<()> {
		let mut state = self.state.lock().unwrap();
		state.committed += 1;
		state.decisions.append(&mut self.decisions);
		self.committed = true;
		Ok(())
	}
}

impl Drop for Transaction {
	fn drop(&mut self) {
		if !self.committed {
			self.state.lock().unwrap().rolled_back += 1;
		}
	}
}

#[rstest::rstest]
#[case(false, true)]
#[case(true, false)]
#[tokio::test]
async fn decision_and_audit_commit_together(#[case] fail_audit: bool, #[case] succeeds: bool) {
	// Arrange: transaction writes stay private until the same transaction commits.
	let state = Arc::new(Mutex::new(State::default()));
	let service = Authorization::new(Arc::new(Store {
		state: state.clone(),
		fail_audit,
	}));

	// Act: the worker identity uses exactly the same use case as an HTTP caller.
	let result = service.evaluate("acme", &evaluation()).await;

	// Assert: an audit failure cannot expose an unaudited successful decision.
	assert_eq!(result.is_ok(), succeeds);
	let state = state.lock().unwrap();
	assert_eq!(state.begun, 1);
	assert_eq!(state.committed, usize::from(succeeds));
	assert_eq!(state.rolled_back, usize::from(!succeeds));
	if let Ok(decision) = result {
		assert!(decision.allowed);
		assert_eq!(decision.revision, 7);
		assert_eq!(state.decisions.len(), 1);
		assert_eq!(state.decisions[0].revision, decision.revision);
	} else {
		assert!(state.decisions.is_empty());
	}
}

#[rstest::rstest]
#[tokio::test]
async fn simulation_has_no_decision_audit() {
	let state = Arc::new(Mutex::new(State::default()));
	let service = Authorization::new(Arc::new(Store {
		state: state.clone(),
		fail_audit: true,
	}));

	let decision = service.simulate("acme", &evaluation()).await.unwrap();

	assert!(decision.allowed);
	assert_eq!(decision.revision, 7);
	assert!(state.lock().unwrap().decisions.is_empty());
}

#[rstest::rstest]
#[tokio::test]
async fn invalid_replacement_never_opens_a_transaction() {
	let state = Arc::new(Mutex::new(State::default()));
	let service = Authorization::new(Arc::new(Store {
		state: state.clone(),
		fail_audit: false,
	}));

	let result = service.replace("other", 7, bundle(), "operator").await;

	assert!(matches!(result, Err(Error::Invalid(_))));
	assert_eq!(state.lock().unwrap().begun, 0);
}

#[rstest::rstest]
#[case("workspace.read", true)]
#[case("workspace.delete", false)]
#[tokio::test]
async fn borrowed_authorization_leaves_commit_to_the_protected_effect(
	#[case] action: &str,
	#[case] allowed: bool,
) {
	// Arrange: the enclosing use case owns both its write and authorization.
	let state = Arc::new(Mutex::new(State::default()));
	let store = Store {
		state: state.clone(),
		fail_audit: false,
	};
	let mut transaction = store.begin().await.unwrap();
	let mut input = evaluation();
	input.action = action.into();

	// Act: evaluating through a borrowed scope cannot finish the transaction.
	let decision = Authorization::evaluate_in(transaction.as_mut(), "acme", &input)
		.await
		.unwrap();

	// Assert: both allow and deny remain audited on the caller's boundary.
	assert_eq!(decision.allowed, allowed);
	assert_eq!(state.lock().unwrap().committed, 0);
	assert!(state.lock().unwrap().decisions.is_empty());
	transaction.commit().await.unwrap();
	let state = state.lock().unwrap();
	assert_eq!(state.committed, 1);
	assert_eq!(state.decisions.len(), 1);
	assert_eq!(state.decisions[0].allowed, allowed);
}
