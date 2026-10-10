//! Deferred exposure tools over a fixture snapshot and recording ports.
use super::*;
use crate::{
	ports::tools::ToolOperations,
	registry::bindings::execution::bound_specification,
	tools::{ToolContext, builtins},
};
use aidash_domain::{
	Artifact, ArtifactInput, HumanRequest, NewTask, ReadyState, RecoveryState, RunControl,
	RunState, StateVersion, Task,
	capabilities::skills::SkillMetadata,
	context::Context,
	exposure::{ExposurePolicy, ExposureUpdate, SkillOriginKind},
	registry::{
		EntityRef, Entry, Search, SkillFile,
		bindings::{BindingSnapshot, SKILL_ASSET_READ},
	},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use uuid::Uuid;

const NODE: &str = "aidash://fixture";
/// Non-UTF-8 bytes of a packaged binary file.
const BINARY: &[u8] = &[0xff, 0xfe, 0x00, 0x01];

struct Operations {
	specifications: BTreeMap<String, ToolSpec>,
	direct: Vec<DirectSkill>,
	files: BTreeMap<String, Vec<u8>>,
	read_bytes: usize,
}
#[async_trait]
impl ToolOperations for Operations {
	async fn registered_skills(&self) -> Result<Vec<EntityRef>> {
		panic!("unexpected tool effect: registered_skills")
	}
	async fn skill_files(&self, _: &EntityRef) -> Result<Vec<SkillFile>> {
		panic!("unexpected tool effect: skill_files")
	}
	async fn discover(&self, _: &Search) -> Result<Value> {
		panic!("unexpected tool effect: discover")
	}
	async fn create_task(&self, _: &str, _: &NewTask) -> Result<Task> {
		panic!("unexpected tool effect: create_task")
	}
	async fn assign(&self, _: Uuid, _: &str, _: &str) -> Result<Value> {
		panic!("unexpected tool effect: assign")
	}
	async fn delegate_with_key(&self, _: &str, _: Uuid, _: &str, _: &EntityRef) -> Result<Value> {
		panic!("unexpected tool effect: delegate")
	}
	async fn artifact(&self, _: &str, _: &ArtifactInput) -> Result<Artifact> {
		panic!("unexpected tool effect: artifact")
	}
	async fn message(&self, _: &str, _: &str) -> Result<()> {
		panic!("unexpected tool effect: message")
	}
	async fn observation(&self, _: usize, _: usize) -> Result<Value> {
		panic!("unexpected tool effect: observation")
	}
	async fn read_record_chunk(&self, _: &str, _: &str, _: usize, _: usize) -> Result<Value> {
		panic!("unexpected tool effect: read_record_chunk")
	}
	async fn memory_mutate(
		&self,
		_: &str,
		_: &[aidash_domain::memory::Change],
	) -> Result<Vec<aidash_domain::memory::Unit>> {
		panic!("unexpected tool effect: memory_mutate")
	}
	async fn memory_recall(
		&self,
		_: &str,
		_: &aidash_domain::memory::RecallQuery,
		_: bool,
	) -> Result<Value> {
		panic!("unexpected tool effect: memory_recall")
	}
	async fn human_request(&self, _: &str, _: &str, _: &str) -> Result<HumanRequest> {
		panic!("unexpected tool effect: human_request")
	}
	async fn binding_specifications(&self) -> Result<BTreeMap<String, ToolSpec>> {
		Ok(self.specifications.clone())
	}
	async fn direct_skills(&self) -> Result<Vec<DirectSkill>> {
		Ok(self.direct.clone())
	}
	async fn direct_skill_file(
		&self,
		skill_id: Uuid,
		digest: &str,
		path: &str,
	) -> Result<Option<Vec<u8>>> {
		let skill = self
			.direct
			.iter()
			.find(|skill| skill.metadata.skill_id == skill_id)
			.ok_or_else(|| Error::NotFound("skill unavailable".into()))?;
		if skill.metadata.digest != digest {
			return Err(Error::Conflict("CAPABILITY_CHANGED".into()));
		}
		self.files
			.get(path)
			.cloned()
			.map(Some)
			.ok_or_else(|| Error::Invalid("SKILL_FILE_UNAVAILABLE".into()))
	}
	fn skill_read_bytes(&self) -> Result<usize> {
		Ok(self.read_bytes)
	}
}

struct Fixture {
	operations: Operations,
	run: Run,
}
impl Fixture {
	async fn call(&self, name: &str, input: Value) -> Result<Value> {
		builtins()[name]
			.invoke(
				&ToolContext {
					operations: &self.operations,
					run: &self.run,
				},
				input,
				"key",
			)
			.await
	}
	async fn capability(&self, alias_prefix: &str) -> Value {
		let found = self.call("capability_search", json!({})).await.unwrap();
		found["results"]
			.as_array()
			.unwrap()
			.iter()
			.find(|item| item["alias"].as_str().unwrap().starts_with(alias_prefix))
			.cloned()
			.unwrap()
	}
	fn apply(&mut self, output: &Value) {
		let update: ExposureUpdate =
			serde_json::from_value(output["exposure_update"].clone()).unwrap();
		self.run.context.exposure.apply(&update);
	}
}

fn run(snapshot: BindingSnapshot) -> Run {
	let now = "2026-10-02T00:00:00Z".parse().unwrap();
	let context = Context {
		binding_snapshot: Some(snapshot.into()),
		..Default::default()
	};
	Run {
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		home_node: NODE.into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		state_version: StateVersion::default(),
		state: RunState::Ready(ReadyState::default()),
		recovery: RecoveryState::default(),
		control: RunControl::Active,
		context,
		step: 3,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: now,
	}
}

fn described(mut entry: Entry, description: &str) -> Entry {
	entry.description = BTreeMap::from([("en".into(), description.into())]);
	entry
}

#[fixture]
fn fixture() -> Fixture {
	use base64::Engine;
	let policy = ExposurePolicy::Deferred(Default::default());
	let mut root = crate::test_support::agent("agent");
	root.config["exposure"] = json!(policy);
	root.config["remove_default"] = json!(
		policy
			.default_tools()
			.into_iter()
			.filter(|name| *name != SKILL_ASSET_READ)
			.collect::<Vec<_>>()
	);
	let bindings = root.config["bindings"].as_array_mut().unwrap();
	let mut extras = vec![];
	for (alias, description) in [
		("alpha_lookup", "Alpha lookup over invoices"),
		("beta_report", "Beta report writer"),
	] {
		bindings.push(crate::test_support::binding("tool", NODE, alias));
		extras.push(described(
			crate::test_support::http_tool(NODE, alias, alias),
			description,
		));
	}
	bindings.push(crate::test_support::binding("skill", NODE, "guide"));
	extras.push(described(
		crate::test_support::entry(
			"guide",
			"skill",
			json!({"instructions":"Follow the guide.","files":[
				{"path":"notes.md","content":"界abc"},
				{"path":"image.bin","content":base64::engine::general_purpose::STANDARD.encode(BINARY),"encoding":"base64"},
				{"path":"data.txt","content":base64::engine::general_purpose::STANDARD.encode("plain text"),"encoding":"base64"},
			]}),
		),
		"Guide Skill",
	));
	let snapshot = crate::test_support::resolve(NODE, &root, false, extras);
	let specifications = snapshot
		.bindings
		.iter()
		.filter_map(|binding| {
			let alias = binding.alias.clone()?;
			let inner = ToolSpec {
				name: binding.identity.id.clone(),
				description: "provider".into(),
				parameters: json!({}),
			};
			Some((alias, bound_specification(binding, inner)))
		})
		.collect();
	let skill_id = Uuid::from_u128(7);
	Fixture {
		operations: Operations {
			specifications,
			direct: vec![DirectSkill {
				metadata: SkillMetadata {
					skill_id,
					name: "Field notes".into(),
					description: "Direct field notes".into(),
					origin: "area:fixture:.agents/skills/notes".into(),
					digest: "direct-digest".into(),
					license: None,
				},
				origin: SkillOriginKind::Root,
				body_bytes: 12,
				files: vec![
					json!({"path":"SKILL.md","digest":"sha256:skill","size":12}),
					json!({"path":"guide.md","digest":"sha256:guide","size":5}),
					json!({"path":"blob.bin","digest":"sha256:blob","size":4}),
				],
			}],
			files: BTreeMap::from([
				("SKILL.md".into(), b"Read notes.".to_vec()),
				("guide.md".into(), b"hello".to_vec()),
				("blob.bin".into(), BINARY.to_vec()),
			]),
			read_bytes: 4,
		},
		run: run(snapshot),
	}
}

#[rstest]
#[tokio::test]
async fn search_and_describe_cover_tools_and_skills_of_the_snapshot(fixture: Fixture) {
	let found = fixture
		.call("capability_search", json!({"query":"alpha invoices"}))
		.await
		.unwrap();
	assert_eq!(found["results"][0]["alias"], "alpha_lookup");
	assert_eq!(found["results"][0]["loaded"], false);
	assert_eq!(found["truncated"], false);
	let all = fixture.call("capability_search", json!({})).await.unwrap();
	let aliases = all["results"]
		.as_array()
		.unwrap()
		.iter()
		.map(|item| item["alias"].as_str().unwrap())
		.collect::<Vec<_>>();
	assert_eq!(aliases.len(), 11);
	assert_eq!(all["next_cursor"], Value::Null);
	let rest = fixture
		.call("capability_search", json!({"cursor":"10"}))
		.await
		.unwrap();
	assert_eq!(rest["results"][0], all["results"][10]);
	let mandatory = all["results"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["alias"] == "capability_load")
		.unwrap();
	assert_eq!(mandatory["loaded"], true);
	let described = fixture
		.call("capability_describe", json!({"alias":"beta_report"}))
		.await
		.unwrap();
	assert_eq!(described["kind"], "tool");
	assert_eq!(
		described["detail"],
		json!(fixture.operations.specifications["beta_report"])
	);
	let skill = fixture.capability("skill_guide").await;
	let described = fixture
		.call("capability_describe", json!({"alias":skill["alias"]}))
		.await
		.unwrap();
	assert_eq!(described["detail"]["files"][0]["path"], "notes.md");
	assert!(matches!(
		fixture.call("capability_describe", json!({"alias":"missing"})).await,
		Err(error) if code(&error) == ("invalid", "UNKNOWN_CAPABILITY")
	));
}

#[rstest]
#[tokio::test]
async fn load_and_unload_return_updates_without_mutating_the_run(mut fixture: Fixture) {
	let beta = fixture.capability("beta_report").await;
	assert!(matches!(
		fixture
			.call("capability_load", json!({"alias":"beta_report","digest":"stale"}))
			.await,
		Err(error) if code(&error) == ("invalid", "CAPABILITY_CHANGED")
	));
	let loaded = fixture
		.call(
			"capability_load",
			json!({"alias":"beta_report","digest":beta["digest"]}),
		)
		.await
		.unwrap();
	assert_eq!(loaded["status"], "loaded");
	assert_eq!(loaded["exposure_update"]["load"]["step"], 3);
	assert!(fixture.run.context.exposure.is_empty());
	fixture.apply(&loaded);
	let again = fixture
		.call(
			"capability_load",
			json!({"alias":"beta_report","digest":beta["digest"]}),
		)
		.await
		.unwrap();
	assert_eq!(again["status"], "already_loaded");
	assert!(again.get("exposure_update").is_none());
	assert_eq!(fixture.capability("beta_report").await["loaded"], true);
	let unloaded = fixture
		.call("capability_unload", json!({"alias":"beta_report"}))
		.await
		.unwrap();
	assert_eq!(unloaded["status"], "unloaded");
	fixture.apply(&unloaded);
	assert_eq!(
		fixture
			.call("capability_unload", json!({"alias":"beta_report"}))
			.await
			.unwrap()["status"],
		"not_loaded"
	);
	assert!(matches!(
		fixture
			.call("capability_unload", json!({"alias":"capability_search"}))
			.await,
		Err(error) if code(&error) == ("invalid", "MANDATORY_EXPOSURE")
	));
}

#[rstest]
#[tokio::test]
async fn a_legacy_run_has_no_discoverable_catalog(mut fixture: Fixture) {
	let legacy =
		crate::test_support::resolve(NODE, &crate::test_support::agent("agent"), false, vec![]);
	fixture.run = run(legacy);
	assert!(matches!(
		fixture.call("capability_search", json!({})).await,
		Err(Error::Invalid(message)) if message.contains("deferred@1")
	));
}

#[rstest]
#[tokio::test]
async fn registry_skill_assets_continue_within_the_read_budget(fixture: Fixture) {
	let skill = fixture.capability("skill_guide").await;
	let read = |offset: Option<usize>| {
		let mut input = json!({"alias":skill["alias"],"digest":skill["digest"],"path":"notes.md"});
		if let Some(offset) = offset {
			input["offset"] = json!(offset);
		}
		input
	};
	let first = fixture.call("skill_asset_read", read(None)).await.unwrap();
	assert_eq!(first["content"], "界a");
	assert_eq!(first["next_offset"], 2);
	assert_eq!(first["truncated"], true);
	let second = fixture
		.call("skill_asset_read", read(Some(2)))
		.await
		.unwrap();
	assert_eq!(second["content"], "bc");
	assert_eq!(second["next_offset"], Value::Null);
	assert_eq!(second["truncated"], false);
	let binary = fixture
		.call(
			"skill_asset_read",
			json!({"alias":skill["alias"],"digest":skill["digest"],"path":"image.bin"}),
		)
		.await
		.unwrap();
	assert_eq!(binary["encoding"], "binary");
	assert_eq!(binary["metadata"]["size"], BINARY.len());
	assert!(binary.get("content").is_none());
	// A declared base64 asset stays binary even when it decodes to UTF-8.
	let declared = fixture
		.call(
			"skill_asset_read",
			json!({"alias":skill["alias"],"digest":skill["digest"],"path":"data.txt"}),
		)
		.await
		.unwrap();
	assert_eq!(declared["encoding"], "binary");
	assert!(declared.get("content").is_none());
	assert!(matches!(
		fixture
			.call(
				"skill_asset_read",
				json!({"alias":skill["alias"],"digest":"changed","path":"notes.md"}),
			)
			.await,
		Err(error) if code(&error) == ("invalid", "CAPABILITY_CHANGED")
	));
	assert!(matches!(
		fixture
			.call(
				"skill_asset_read",
				json!({"alias":"beta_report","digest":"x","path":"notes.md"}),
			)
			.await,
		Err(error) if code(&error) == ("invalid", "UNKNOWN_CAPABILITY")
	));
	assert!(matches!(
		fixture
			.call(
				"skill_asset_read",
				json!({"alias":skill["alias"],"digest":skill["digest"],"path":"missing.md"}),
			)
			.await,
		Err(error) if code(&error) == ("invalid", "SKILL_FILE_UNAVAILABLE")
	));
}

/// Domain and use-case errors reach the Executor alike.
fn code(error: &Error) -> (&'static str, &str) {
	match error {
		Error::Invalid(code) | Error::Domain(aidash_domain::Error::Invalid(code)) => {
			("invalid", code)
		}
		Error::Conflict(code) | Error::Domain(aidash_domain::Error::Conflict(code)) => {
			("conflict", code)
		}
		_ => ("other", ""),
	}
}
#[rstest]
#[tokio::test]
async fn direct_skill_assets_read_through_the_pinned_record(fixture: Fixture) {
	let skill = fixture.capability("skill_field_notes").await;
	assert_eq!(skill["digest"], "direct-digest");
	let read = |path: &str, offset: usize| json!({"alias":skill["alias"],"digest":skill["digest"],"path":path,"offset":offset,"max_chars":3});
	let first = fixture
		.call("skill_asset_read", read("guide.md", 0))
		.await
		.unwrap();
	assert_eq!(first["content"], "hel");
	assert_eq!(first["digest"], "sha256:guide");
	assert_eq!(first["next_offset"], 3);
	let second = fixture
		.call("skill_asset_read", read("guide.md", 3))
		.await
		.unwrap();
	assert_eq!(second["content"], "lo");
	assert_eq!(second["truncated"], false);
	let binary = fixture
		.call("skill_asset_read", read("blob.bin", 0))
		.await
		.unwrap();
	assert_eq!(binary["encoding"], "binary");
	assert_eq!(binary["metadata"]["digest"], "sha256:blob");
	assert!(matches!(
		fixture
			.call("skill_asset_read", read("missing.md", 0))
			.await,
		Err(error) if code(&error) == ("invalid", "SKILL_FILE_UNAVAILABLE")
	));
	assert!(matches!(
		fixture
			.call(
				"skill_asset_read",
				json!({"alias":skill["alias"],"digest":"stale","path":"guide.md"}),
			)
			.await,
		Err(error) if code(&error) == ("invalid", "CAPABILITY_CHANGED")
	));
}
