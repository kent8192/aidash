use super::*;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Configuration {
	calls: Arc<Mutex<Vec<&'static str>>>,
	fail: Option<&'static str>,
	old: Option<Index>,
	counts: (i64, usize),
	committed: bool,
}
impl Configuration {
	fn with_old(mut self, old: Index) -> Self {
		self.old = Some(old);
		self
	}
	fn with_failure(mut self, failure: &'static str) -> Self {
		self.fail = Some(failure);
		self
	}
	fn with_counts(mut self, counts: (i64, usize)) -> Self {
		self.counts = counts;
		self
	}
	fn touch(&self, name: &'static str) -> Result<()> {
		self.calls.lock().unwrap().push(name);
		if self.fail == Some(name) {
			return Err(Error::External(name.into()));
		}
		Ok(())
	}
}
struct Repository(Configuration);
#[async_trait]
impl SemanticConfigurationRepository for Repository {
	fn validate(&self, _: &IndexingSpec) -> Result<()> {
		self.0.touch("validate")
	}
	async fn begin(&self) -> Result<Box<dyn SemanticConfigurationSession>> {
		self.0.touch("begin")?;
		Ok(Box::new(self.0.clone()))
	}
}
impl Drop for Configuration {
	fn drop(&mut self) {
		if !self.committed {
			self.calls.lock().unwrap().push("rollback");
		}
	}
}
#[async_trait]
impl SemanticConfigurationSession for Configuration {
	async fn lock_workspace(&mut self, _: Uuid) -> Result<()> {
		self.touch("workspace")
	}
	async fn tenant(&mut self, _: Uuid) -> Result<String> {
		self.touch("tenant")?;
		Ok("tenant".into())
	}
	async fn index(&mut self, _: Uuid) -> Result<Option<Index>> {
		self.touch("index")?;
		Ok(self.old.clone())
	}
	async fn counts(&mut self, _: Uuid) -> Result<(i64, usize)> {
		self.touch("counts")?;
		Ok(self.counts)
	}
	async fn replace(
		&mut self,
		workspace: Uuid,
		tenant: &str,
		revision: i64,
		spec: Value,
		collection: &str,
	) -> Result<Index> {
		self.touch("replace")?;
		assert_eq!(tenant, "tenant");
		assert!(collection.starts_with("aidash_"));
		let mut index = index();
		index.workspace_id = workspace;
		index.revision = revision;
		index.spec = spec;
		index.collection = collection.into();
		Ok(index)
	}
	async fn retire_collections(&mut self, _: Uuid) -> Result<()> {
		self.touch("retire")
	}
	async fn record_collection(&mut self, _: Uuid, collection: &str, vector: Value) -> Result<()> {
		self.touch("record")?;
		assert!(collection.starts_with("aidash_"));
		assert_eq!(vector, serde_json::to_value(spec().vector).unwrap());
		Ok(())
	}
	async fn active(&mut self, _: Uuid) -> Result<Vec<Uuid>> {
		self.touch("active")?;
		Ok(vec![Uuid::from_u128(2)])
	}
	async fn reconfigure(&mut self, id: Uuid, revision: i64) -> Result<Entry> {
		self.touch("reconfigure")?;
		let mut entry = entry();
		entry.id = id;
		entry.index_revision = revision;
		Ok(entry)
	}
	async fn schedule(&mut self, entry: &Entry, _: &str) -> Result<()> {
		self.touch("schedule")?;
		assert_eq!(entry.index_revision, 1);
		Ok(())
	}
	async fn history(&mut self, _: Uuid, revision: i64) -> Result<()> {
		self.touch("history")?;
		assert_eq!(revision, 1);
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		self.touch("commit")?;
		self.committed = true;
		Ok(())
	}
}

#[rstest]
#[tokio::test]
async fn configuring_commits_one_complete_immutable_generation() {
	let repository = Repository(Configuration::default());
	let result = configure(&repository, Uuid::from_u128(1), 0, spec())
		.await
		.unwrap();
	assert_eq!(result.revision, 1);
	assert_eq!(result.spec, serde_json::to_value(spec()).unwrap());
	assert_eq!(
		*repository.0.calls.lock().unwrap(),
		[
			"validate",
			"begin",
			"workspace",
			"tenant",
			"index",
			"counts",
			"replace",
			"retire",
			"record",
			"active",
			"reconfigure",
			"schedule",
			"history",
			"commit"
		]
	);
}
#[rstest]
#[tokio::test]
async fn identical_config_replays_before_size_checks_or_point_retirement() {
	let repository = Repository(
		Configuration::default()
			.with_old(Index {
				revision: 1,
				..index()
			})
			.with_counts((i64::MAX, usize::MAX)),
	);
	let result = configure(&repository, Uuid::from_u128(1), 0, spec())
		.await
		.unwrap();
	assert_eq!(result.revision, 1);
	assert_eq!(result.collection, "collection");
	assert_eq!(
		*repository.0.calls.lock().unwrap(),
		[
			"validate",
			"begin",
			"workspace",
			"tenant",
			"index",
			"commit"
		]
	);
}
#[rstest]
#[case::validate("validate")]
#[case::begin("begin")]
#[case::workspace("workspace")]
#[case::tenant("tenant")]
#[case::index("index")]
#[case::counts("counts")]
#[case::replace("replace")]
#[case::retire("retire")]
#[case::record("record")]
#[case::active("active")]
#[case::reconfigure("reconfigure")]
#[case::schedule("schedule")]
#[case::history("history")]
#[case::commit("commit")]
#[tokio::test]
async fn failed_configuration_does_not_commit_partial_generations(#[case] failure: &'static str) {
	let repository = Repository(Configuration::default().with_failure(failure));
	assert_eq!(
		configure(&repository, Uuid::from_u128(1), 0, spec())
			.await
			.unwrap_err()
			.to_string(),
		failure
	);
	let calls = repository.0.calls.lock().unwrap();
	if matches!(failure, "validate" | "begin") {
		assert_eq!(calls.last(), Some(&failure));
	} else {
		assert_eq!(calls.last(), Some(&"rollback"));
	}
	if failure != "commit" {
		assert!(!calls.contains(&"commit"));
	}
}
#[rstest]
#[case::negative(-1)]
#[case::overflow(i64::MAX)]
#[tokio::test]
async fn invalid_revision_never_opens_a_database_transaction(#[case] revision: i64) {
	let repository = Repository(Configuration::default());
	assert_eq!(
		configure(&repository, Uuid::from_u128(1), revision, spec())
			.await
			.unwrap_err()
			.to_string(),
		"invalid index revision"
	);
	assert_eq!(*repository.0.calls.lock().unwrap(), ["validate"]);
}
#[rstest]
#[case::sources((9,0),"index limit is below the current source count")]
#[case::bytes((0,129),"semantic index max_input_bytes is below existing memory text")]
#[tokio::test]
async fn shared_index_lock_fences_configuration_against_existing_sources(
	#[case] counts: (i64, usize),
	#[case] reason: &str,
) {
	let repository = Repository(Configuration::default().with_counts(counts));
	assert_eq!(
		configure(&repository, Uuid::from_u128(1), 0, spec())
			.await
			.unwrap_err()
			.to_string(),
		reason
	);
	assert_eq!(
		*repository.0.calls.lock().unwrap(),
		[
			"validate",
			"begin",
			"workspace",
			"tenant",
			"index",
			"counts",
			"rollback"
		]
	);
}
#[rstest]
#[tokio::test]
async fn stale_configuration_does_not_replace_another_generation() {
	let repository = Repository(Configuration::default().with_old(index()));
	assert_eq!(
		configure(&repository, Uuid::from_u128(1), 0, spec())
			.await
			.unwrap_err()
			.to_string(),
		"semantic index revision changed"
	);
	assert_eq!(
		*repository.0.calls.lock().unwrap(),
		[
			"validate",
			"begin",
			"workspace",
			"tenant",
			"index",
			"rollback"
		]
	);
}
