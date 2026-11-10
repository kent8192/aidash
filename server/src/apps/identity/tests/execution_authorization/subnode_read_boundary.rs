//! The boundary between a Subnode's Workspace and the Workspace of the Run that
//! created it, in both directions (#750, #755, ADR 0110).
//!
//! The creator here starts from a conversation, and agents hold what a Tenant's
//! members hold, not the fixture's `*`.
use super::*;

const MEMBERS: [&str; 3] = ["subnode-leader", "subnode-second", "delegate-target"];

impl SubnodeRig {
	/// Like [`SubnodeRig::new`], with the creator started from a conversation
	/// under a policy that gives agents what members hold.
	async fn from_conversation(
		f: &aidash_server::federation::Federation,
		app: &common::TestApplication,
	) -> Self {
		let (mut policy, token, node, _) = Self::register(f, app, &MEMBERS, true, false).await;
		policy["policies"][0]["actions"] = json!(aidash_domain::identity::tenancy::MEMBER_ACTIONS);
		set_policy(f, app, &policy).await;
		let (status, created) = request(
			app,
			&token,
			"POST",
			"/api/conversations",
			json!({"title":"Coordinate","goal":"Coordinate the work","target":{"id":"subnode-creator","version":"1.0.0"},"target_kind":"agent"}),
		)
		.await;
		assert_eq!(status, 200, "conversation: {created}");
		let workspace = Uuid::parse_str(created["workspace"]["id"].as_str().unwrap()).unwrap();
		let creator_run = f
			.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.workspace_id == workspace)
			.expect("the conversation started the creator")
			.id;
		Self {
			f: f.clone(),
			app: app.clone(),
			token,
			node,
			creator_run,
			policy,
			epoch: std::cell::Cell::new(0),
		}
	}

	/// Call a tool as `run` while every other Run stays paused, so the worker
	/// selects this one, and return its result. A refused call leaves no result,
	/// and the panic names why the Run stopped.
	async fn call(&self, run: Uuid, name: &str, arguments: Value, call_id: &str) -> Value {
		sqlx::query(
			"UPDATE runs SET control = CASE WHEN id = $1 THEN 'ACTIVE' ELSE 'PAUSED' END \
			 WHERE control IN ('ACTIVE', 'PAUSED')",
		)
		.bind(run)
		.execute(self.f.store.pool.fixture_driver())
		.await
		.unwrap();
		assert!(
			self.script(run, name, arguments, call_id).await,
			"{call_id}: the worker took no step"
		);
		// A Run that was cancelled may have taken that step to finish.
		let harness = Harness {
			federation: self.f.clone(),
		};
		for _ in 0..4 {
			let current = self.f.store.run(&TEST_FIXTURE, run).await.unwrap();
			if current.control == aidash_domain::RunControl::Paused
				|| self.result_of(run, call_id).await.is_some()
			{
				break;
			}
			harness.worker_once().await.unwrap();
		}
		match self.result_of(run, call_id).await {
			Some(result) => {
				assert!(result.get("error").is_none(), "{call_id}: {result}");
				result
			}
			None => {
				let stopped = self.f.store.run(&TEST_FIXTURE, run).await.unwrap();
				panic!(
					"{call_id} has no result: control {:?}, error {:?}",
					stopped.control, stopped.error
				)
			}
		}
	}

	/// Read the Run's own Task, as a Run started from a conversation does before
	/// it does anything else. This records a read in its own Workspace.
	async fn read_own_task(&self, run: Uuid) {
		let task = self.f.store.run(&TEST_FIXTURE, run).await.unwrap().task_id;
		self.call(
			run,
			"workspace_read",
			json!({"kind":"task","id":task}),
			&format!("read-own-task-{run}"),
		)
		.await;
	}

	/// Create a Subnode led by `subnode-leader` and give it a Task. Returns its
	/// id, its Workspace, the Task and the leader's Run.
	async fn subnode_with_task(&self) -> (Value, Uuid, Uuid, Uuid) {
		let created = self
			.call(
				self.creator_run,
				"subnode_create",
				json!({
					"leader":"subnode-leader@1.0.0",
					"members":["subnode-leader@1.0.0","subnode-second@1.0.0"],
					"allowance":{"tools":SUBNODE_TOOLS,"secrets":[]},
					"title":"Investigation",
				}),
				"create-subnode",
			)
			.await;
		let workspace = Uuid::parse_str(created["workspace_id"].as_str().unwrap()).unwrap();
		let given = self
			.call(
				self.creator_run,
				"subnode_task",
				json!({"subnode_id":created["subnode_id"],"title":"Member work","request":"Find evidence"}),
				"subnode-task",
			)
			.await;
		let task = Uuid::parse_str(given["task_id"].as_str().unwrap()).unwrap();
		(
			created["subnode_id"].clone(),
			workspace,
			task,
			self.run_of_task(task).await,
		)
	}

	async fn run_of_task(&self, task: Uuid) -> Uuid {
		self.f
			.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id == task)
			.expect("the Task's Run was admitted")
			.id
	}

	async fn create_task(&self, run: Uuid, title: &str, dependencies: &[Uuid]) -> Uuid {
		let created = self
			.call(
				run,
				"task_create",
				json!({"title":title,"description":"Delegated work","dependencies":dependencies}),
				&format!("create-{title}"),
			)
			.await;
		Uuid::parse_str(
			created["id"]
				.as_str()
				.unwrap_or_else(|| panic!("{created}")),
		)
		.unwrap()
	}

	/// Delegate `task` from `run` and return the new Run.
	async fn delegate(&self, run: Uuid, task: Uuid, agent: &str) -> Uuid {
		self.call(
			run,
			"task_delegate",
			json!({"task_id":task,"node_id":self.node.to_string(),"agent":{"id":agent,"version":"1.0.0"}}),
			&format!("delegate-{task}"),
		)
		.await;
		self.run_of_task(task).await
	}
}

async fn set_policy(
	f: &aidash_server::federation::Federation,
	app: &common::TestApplication,
	policy: &Value,
) {
	let revision: i64 =
		sqlx::query_scalar("SELECT revision FROM authorization_bundles WHERE tenant = 'acme'")
			.fetch_one(f.store.pool.fixture_driver())
			.await
			.unwrap();
	let (status, body) = request(
		app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":revision,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "policy: {body}");
}

/// #755: `subnode_task` records a read in the Subnode's Workspace, and the next
/// delegation checks that read under an authority that names only the creator's
/// Workspace.
#[rstest::rstest]
#[tokio::test]
async fn creator_delegates_after_giving_its_subnode_a_task(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	rig.read_own_task(rig.creator_run).await;
	rig.subnode_with_task().await;
	let first = rig.create_task(rig.creator_run, "first", &[]).await;
	rig.delegate(rig.creator_run, first, "delegate-target")
		.await;
	// A dependency on the creator's own output walks the same reads.
	rig.create_task(rig.creator_run, "second", &[first]).await;
	cleanup(f, &url, &schema).await;
}

/// ADR 0010 lets calls of one step share an authority, so the Subnode's Task and
/// the delegation can arrive together.
#[rstest::rstest]
#[tokio::test]
async fn creator_delegates_in_the_same_step_as_giving_its_subnode_a_task(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	rig.read_own_task(rig.creator_run).await;
	let created = rig
		.call(
			rig.creator_run,
			"subnode_create",
			json!({"leader":"subnode-leader@1.0.0","members":["subnode-leader@1.0.0"],"allowance":{"tools":SUBNODE_TOOLS,"secrets":[]}}),
			"create-subnode",
		)
		.await;
	let task = rig.create_task(rig.creator_run, "together", &[]).await;
	let response = json!({"text":"","tool_calls":[
		{"id":"give","name":"subnode_task","arguments":{"subnode_id":created["subnode_id"],"title":"Member work","request":"Find evidence"}},
		{"id":"hand","name":"task_delegate","arguments":{"task_id":task,"node_id":rig.node.to_string(),"agent":{"id":"delegate-target","version":"1.0.0"}}},
	],"input_tokens":0,"output_tokens":0});
	rig.epoch.set(rig.epoch.get() + 1);
	let pending = common::tool_pending(
		json!({"response":response,"response_epoch":rig.epoch.get(),"cursor":0,"request_window":120000,"request_tokens":0}),
	);
	sqlx::query("UPDATE runs SET phase = 'TOOL_CALL', pending = $1 WHERE id = $2")
		.bind(pending)
		.bind(rig.creator_run)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let harness = Harness {
		federation: f.clone(),
	};
	// The leader's Run appears after the first call: keep it from taking a step.
	for _ in 0..4 {
		harness.worker_once().await.unwrap();
		sqlx::query("UPDATE runs SET control = 'PAUSED' WHERE id <> $1 AND control = 'ACTIVE'")
			.bind(rig.creator_run)
			.execute(f.store.pool.fixture_driver())
			.await
			.unwrap();
		if rig.result_of(rig.creator_run, "hand").await.is_some() {
			break;
		}
	}
	let stopped = f.store.run(&TEST_FIXTURE, rig.creator_run).await.unwrap();
	for call in ["give", "hand"] {
		let result = rig
			.result_of(rig.creator_run, call)
			.await
			.unwrap_or_else(|| {
				panic!(
					"{call} has no result: control {:?}, error {:?}",
					stopped.control, stopped.error
				)
			});
		assert!(result.get("error").is_none(), "{call}: {result}");
	}
	cleanup(f, &url, &schema).await;
}

/// ADR 0110 keeps a Subnode's Workspace with the creator's records once it ends.
#[rstest::rstest]
#[tokio::test]
async fn creator_delegates_after_its_subnode_ended(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	rig.read_own_task(rig.creator_run).await;
	let (subnode, ..) = rig.subnode_with_task().await;
	rig.call(
		rig.creator_run,
		"subnode_dissolve",
		json!({"subnode_id":subnode}),
		"dissolve",
	)
	.await;
	let task = rig.create_task(rig.creator_run, "after", &[]).await;
	rig.delegate(rig.creator_run, task, "delegate-target").await;
	cleanup(f, &url, &schema).await;
}

/// A Run the creator delegated to reads a Task the creator produced, so its
/// checks walk the creator's reads too. It still cannot read the Subnode's
/// Workspace itself.
#[rstest::rstest]
#[tokio::test]
async fn run_the_creator_delegated_to_passes_the_check_but_cannot_read_the_subnode(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	rig.read_own_task(rig.creator_run).await;
	let helper_task = rig.create_task(rig.creator_run, "helper", &[]).await;
	let helper = rig
		.delegate(rig.creator_run, helper_task, "delegate-target")
		.await;
	let (_, workspace, root, _) = rig.subnode_with_task().await;
	rig.read_own_task(helper).await;
	let task = rig.create_task(helper, "from-helper", &[]).await;
	rig.delegate(helper, task, "subnode-second").await;
	let (status, body) = request(
		&rig.app,
		&rig.token,
		"GET",
		&format!("/api/tasks/{helper_task}/runs"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "the owner reads the helper's Task: {body}");
	rig.script(
		helper,
		"workspace_read",
		json!({"workspace_id":workspace,"kind":"task","id":root}),
		"helper-names-subnode",
	)
	.await;
	assert!(
		rig.result_of(helper, "helper-names-subnode")
			.await
			.is_none(),
		"only the creator reads the Subnode's Workspace"
	);
	cleanup(f, &url, &schema).await;
}

/// #750: the Task a creator gives a Subnode reaches its members, although they
/// cannot read what the creator had read.
#[rstest::rstest]
#[tokio::test]
async fn member_creates_and_delegates_tasks_when_its_creator_has_read_its_own_workspace(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	rig.read_own_task(rig.creator_run).await;
	let (.., leader) = rig.subnode_with_task().await;
	rig.read_own_task(leader).await;
	let task = rig.create_task(leader, "member-task", &[]).await;
	let member = rig.delegate(leader, task, "subnode-second").await;
	rig.read_own_task(member).await;
	cleanup(f, &url, &schema).await;
}

/// Only a Run inside the Subnode skips the creator's reads: a person who may not
/// read the creator's conversation stays refused on the Task it gave the Subnode.
#[rstest::rstest]
#[tokio::test]
async fn person_who_cannot_read_the_creators_records_stays_refused_on_the_subnodes_task(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	rig.read_own_task(rig.creator_run).await;
	let (_, _, root, _) = rig.subnode_with_task().await;
	let creator_task = f
		.store
		.run(&TEST_FIXTURE, rig.creator_run)
		.await
		.unwrap()
		.task_id;
	let mut policy = rig.policy.clone();
	policy["subjects"]["bob"] = json!({"kind":"user"});
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"bob-cannot-read-what-the-creator-read",
		"effect":"deny",
		"subjects":{"ids":["bob"]},
		"actions":["task.read"],
		"resources":{"kinds":["task"],"ids":[creator_task]},
	}));
	set_policy(&f, &rig.app, &policy).await;
	let credential = common::issued_credential(&f.store, "acme", "bob").await;
	let bob = credential["token"].as_str().unwrap();
	let (status, body) = request(
		&rig.app,
		&rig.token,
		"GET",
		&format!("/api/tasks/{root}/runs"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "the owner reads the Subnode's Task: {body}");
	let (status, body) = request(
		&rig.app,
		bob,
		"GET",
		&format!("/api/tasks/{root}/runs"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 403, "bob: {body}");
	cleanup(f, &url, &schema).await;
}

/// Access to what the creator read is withdrawn after it gave the Subnode a
/// Task. Members never checked that read, so they keep working, while the creator
/// and people lose sight of the Task. Returns the Subnode and a member's Run.
async fn withdraw_what_the_creator_read(rig: &SubnodeRig) -> (Value, Uuid) {
	let creator = rig.creator_run;
	rig.read_own_task(creator).await;
	rig.call(creator, "agent_discover", json!({}), "creator-discovers")
		.await;
	let (subnode, workspace, root, leader) = rig.subnode_with_task().await;
	let mut policy = rig.policy.clone();
	policy["subjects"]["carol"] = json!({"kind":"user"});
	set_policy(&rig.f, &rig.app, &policy).await;
	let credential = common::issued_credential(&rig.f.store, "acme", "carol").await;
	let carol = credential["token"].as_str().unwrap();
	let runs = format!("/api/tasks/{root}/runs");
	let (status, body) = request(&rig.app, carol, "GET", &runs, Value::Null).await;
	assert_eq!(status, 200, "carol before the withdrawal: {body}");
	// Catalog Approval governs use, not read dependencies (#769). Deny the
	// discovered definition instead to withdraw the creator's read.
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"hide-discovered-agent", "effect":"deny", "subjects":{"any":true},
		"actions":["registry.read"], "resources":{"kinds":["agent"],"ids":["delegate-target"]},
	}));
	set_policy(&rig.f, &rig.app, &policy).await;
	let (status, body) = request(&rig.app, carol, "GET", &runs, Value::Null).await;
	assert_eq!(status, 403, "carol after the withdrawal: {body}");
	rig.set_run_control(leader, "PAUSED").await;
	rig.set_run_control(creator, "ACTIVE").await;
	rig.script(
		creator,
		"workspace_read",
		json!({"workspace_id":workspace,"kind":"task","id":root}),
		"creator-after-withdrawal",
	)
	.await;
	assert!(
		rig.result_of(creator, "creator-after-withdrawal")
			.await
			.is_none(),
		"the creator lost sight of the Task"
	);
	rig.read_own_task(leader).await;
	let task = rig.create_task(leader, "after-withdrawal", &[]).await;
	let member = rig.delegate(leader, task, "subnode-second").await;
	rig.read_own_task(member).await;
	(subnode, member)
}

/// A person stops those members by dissolving the Subnode.
#[rstest::rstest]
#[tokio::test]
async fn dissolving_the_subnode_stops_members_that_outlived_the_creators_read(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	let (subnode, member) = withdraw_what_the_creator_read(&rig).await;
	let (status, body) = request(
		&rig.app,
		&rig.token,
		"POST",
		&format!("/api/subnodes/{}/dissolve", subnode.as_str().unwrap()),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "dissolve: {body}");
	let stopped = f.store.run(&TEST_FIXTURE, member).await.unwrap();
	assert_eq!(stopped.control, aidash_domain::RunControl::Cancelled);
	cleanup(f, &url, &schema).await;
}

/// A person stops those members by cancelling the creating Run. `/control` still
/// refuses this until #753, so this uses `/management`.
#[rstest::rstest]
#[tokio::test]
async fn cancelling_the_creator_stops_members_that_outlived_its_read(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	let (_, member) = withdraw_what_the_creator_read(&rig).await;
	let (status, body) = request(
		&rig.app,
		&rig.token,
		"POST",
		&format!("/api/runs/{}/management", rig.creator_run),
		json!({"action":"cancel"}),
	)
	.await;
	assert_eq!(status, 200, "cancel: {body}");
	let stopped = f.store.run(&TEST_FIXTURE, member).await.unwrap();
	assert_eq!(stopped.control, aidash_domain::RunControl::Cancelled);
	cleanup(f, &url, &schema).await;
}

/// What a creator is there for: it reads a member's output, and then hands more
/// work on.
async fn creator_reads_a_members_output_and_then_delegates(
	f: aidash_server::federation::Federation,
	app: &common::TestApplication,
	member_discovers: bool,
) {
	let rig = SubnodeRig::from_conversation(&f, app).await;
	rig.read_own_task(rig.creator_run).await;
	let (_, workspace, _, leader) = rig.subnode_with_task().await;
	rig.read_own_task(leader).await;
	if member_discovers {
		rig.call(leader, "agent_discover", json!({}), "member-discovers")
			.await;
	}
	let output = rig.create_task(leader, "member-output", &[]).await;
	let read = rig
		.call(
			rig.creator_run,
			"workspace_read",
			json!({"workspace_id":workspace,"kind":"task","id":output}),
			"creator-reads-member",
		)
		.await;
	assert!(read.to_string().contains("member-output"), "read: {read}");
	let task = rig.create_task(rig.creator_run, "next", &[]).await;
	rig.delegate(rig.creator_run, task, "delegate-target").await;
}

#[rstest::rstest]
#[tokio::test]
async fn creator_reads_the_output_of_a_member_that_discovered_nothing(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	creator_reads_a_members_output_and_then_delegates(f.clone(), &fixture.application, false).await;
	cleanup(f, &url, &schema).await;
}

/// The member discovered agents the creator never used.
#[rstest::rstest]
#[tokio::test]
async fn creator_reads_the_output_of_a_member_that_discovered_other_agents(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	creator_reads_a_members_output_and_then_delegates(f.clone(), &fixture.application, true).await;
	cleanup(f, &url, &schema).await;
}

/// The same shape without a Subnode: a parent reads what a child produced after
/// the child discovered agents the parent never used, and then delegates.
#[rstest::rstest]
#[tokio::test]
async fn parent_delegates_after_reading_a_child_that_discovered_other_agents(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	let parent = rig.creator_run;
	rig.read_own_task(parent).await;
	let child_task = rig.create_task(parent, "child", &[]).await;
	let child = rig.delegate(parent, child_task, "delegate-target").await;
	rig.call(child, "agent_discover", json!({}), "child-discovers")
		.await;
	let output = rig.create_task(child, "child-output", &[]).await;
	let read = rig
		.call(
			parent,
			"workspace_read",
			json!({"kind":"task","id":output}),
			"parent-reads-child",
		)
		.await;
	assert!(read.to_string().contains("child-output"), "read: {read}");
	let next = rig.create_task(parent, "next", &[]).await;
	rig.delegate(parent, next, "subnode-second").await;
	cleanup(f, &url, &schema).await;
}

/// Without a Subnode and without reading first: nothing has yet checked what the
/// child's Run pinned or discovered when the parent's next call checks it.
#[rstest::rstest]
#[tokio::test]
async fn parent_depends_on_a_task_its_child_created_after_discovering_other_agents(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let rig = SubnodeRig::from_conversation(&f, &fixture.application).await;
	let parent = rig.creator_run;
	rig.read_own_task(parent).await;
	let child_task = rig.create_task(parent, "child", &[]).await;
	let child = rig.delegate(parent, child_task, "delegate-target").await;
	rig.call(child, "agent_discover", json!({}), "child-discovers")
		.await;
	let output = rig.create_task(child, "child-output", &[]).await;
	rig.create_task(parent, "next", &[output]).await;
	cleanup(f, &url, &schema).await;
}
