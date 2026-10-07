//! Native memory composition; every public operation acquires a current authority lease.
pub use super::super::repositories::bank_settings::Settings as BankSettings;
use super::super::repositories::{access::Lease, native_memory as repository, units};
use crate::{
	Error, Result, authorization::identity::Actor, database::native, federation::Federation,
	store::Store,
};
use aidash_domain::{memory::*, registry::EntityRef};
use reinhardt::{
	injectable,
	query::{Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone)]
pub struct NativeMemory {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> NativeMemory {
	NativeMemory { runtime }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadBank {
	pub provider: EntityRef,
	pub bank: Bank,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateParticipant {
	pub agent: EntityRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpgradeParticipant {
	pub agent: EntityRef,
	pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Participant {
	pub id: Uuid,
	pub bank: Bank,
	pub agent: EntityRef,
	pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Operation {
	pub operation_id: Uuid,
	pub provider: EntityRef,
	pub bank: Bank,
	pub action: Action,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
	Recall {
		query: RecallQuery,
	},
	Reflect {
		query: RecallQuery,
	},
	Retain {
		text: String,
		evidence: Vec<Evidence>,
	},
	Learn {
		run: Evidence,
	},
	Derive {
		mutation: Mutation,
		kind: Kind,
		sources: Vec<Evidence>,
	},
	Review {
		id: Uuid,
		expected_revision: i64,
		mutation: Option<Mutation>,
	},
	Publish {
		source: Evidence,
		mutation: Mutation,
	},
	ConfigureBank {
		expected_revision: i64,
	},
	Settings,
	Jobs {
		after: Option<Uuid>,
	},
	Usage {
		after: Option<Uuid>,
	},
	Reindex {
		expected_index_revision: i64,
	},
	Candidates,
	Inspect {
		after: Option<Uuid>,
	},
	History {
		id: Uuid,
		expected_revision: i64,
		before: Option<i64>,
	},
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub enum Outcome {
	Recall(Recall),
	Reflection(Reflection),
	Units(Vec<Unit>),
	Candidates(Vec<Candidate>),
	Reviewed(Option<Box<Unit>>),
	Settings(Option<BankSettings>),
	Inspection(super::memory_administration::Inspection),
	History(Vec<super::memory_administration::History>),
	Jobs(super::memory_administration::JobPage),
	Usage(super::memory_administration::UsagePage),
}
impl Action {
	fn permission(&self) -> &'static str {
		match self {
			Self::Recall { .. } => "memory.read",
			Self::Reflect { .. } => "memory.reflect",
			Self::Retain { .. } => "memory.write",
			Self::Learn { .. } => "memory.candidate.propose",
			Self::Derive { .. } => "memory.derive",
			Self::Review { .. } => "memory.candidate.review",
			Self::Publish { .. } => "memory.publish",
			Self::ConfigureBank { .. } => "memory.configure",
			Self::Settings => "memory.read",
			Self::Reindex { .. } => "memory.index.manage",
			Self::Candidates => "memory.candidate.read",
			Self::Inspect { .. }
			| Self::History { .. }
			| Self::Jobs { .. }
			| Self::Usage { .. } => "memory.history.read",
		}
	}
	fn writes(&self) -> bool {
		!matches!(
			self,
			Self::Recall { .. }
				| Self::Reflect { .. }
				| Self::Candidates
				| Self::Settings
				| Self::Inspect { .. }
				| Self::History { .. }
				| Self::Jobs { .. }
				| Self::Usage { .. }
		)
	}
}

pub async fn operate(store: &Store, actor: &Actor, input: Operation) -> Result<Outcome> {
	if matches!(
		input.action,
		Action::Settings
			| Action::Inspect { .. }
			| Action::History { .. }
			| Action::Jobs { .. }
			| Action::Usage { .. }
	) {
		let mut lease = Lease::begin(store, actor).await?;
		let result = operate_in(store, &mut lease, input, None).await;
		return lease.finish(result).await;
	}
	// Establish the empty bank before the independently committed model ledger
	// can reference it. No source knowledge is admitted by this initialization.
	let mut initialization = Lease::begin(store, actor).await?;
	let initialized = async {
		repository::lock_workspace(&mut initialization, input.bank.workspace, true).await?;
		scope(
			store,
			&mut initialization,
			&input.bank,
			input.action.permission(),
		)
		.await?;
		if !matches!(input.action, Action::ConfigureBank { .. }) {
			bank_provider(&mut initialization, &input.bank, &input.provider).await?;
		}
		if input.bank.participant.is_none()
			&& super::super::repositories::bank_settings::get(&mut initialization, &input.bank)
				.await?
				.is_none()
		{
			scope(store, &mut initialization, &input.bank, "memory.configure").await?;
		}
		policy(&mut initialization, &input.provider).await?;
		repository::bank_id(&mut initialization, &input.bank, true).await?;
		if !matches!(input.action, Action::ConfigureBank { .. }) {
			super::super::repositories::bank_settings::ensure(
				&mut initialization,
				&input.bank,
				&input.provider,
			)
			.await?;
		}
		Ok(())
	}
	.await;
	initialization.finish(initialized).await?;
	let mut lease = Lease::begin(store, actor).await?;
	let result = operate_in(store, &mut lease, input, None).await;
	lease.finish(result).await
}
pub(crate) async fn operate_in(
	store: &Store,
	lease: &mut Lease<'_>,
	input: Operation,
	run: Option<Uuid>,
) -> Result<Outcome> {
	use super::super::repositories::{candidates, memory_scope::Scope, publications};
	use aidash_application::ports::memory::MemoryScope;
	repository::lock_workspace(lease, input.bank.workspace, input.action.writes()).await?;
	scope(store, lease, &input.bank, input.action.permission()).await?;
	if !matches!(
		input.action,
		Action::ConfigureBank { .. } | Action::Settings
	) {
		bank_provider(lease, &input.bank, &input.provider).await?;
	}
	let policy = policy(lease, &input.provider).await?;
	let digest = aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(&input)?);
	let check_mutation = |mutation: &Mutation| -> Result<()> {
		if mutation.provider != input.provider
			|| mutation.bank != input.bank
			|| mutation.operation_id != input.operation_id
		{
			return Err(Error::Invalid(
				"nested memory mutation differs from its operation binding".into(),
			));
		}
		Ok(())
	};
	match &input.action {
		Action::ConfigureBank { expected_revision } => {
			candidates::human(lease)?;
			// Private policies follow the explicit Agent version upgrade; shared
			// policies can be changed only by this observed-revision operation.
			if input.bank.participant.is_some() {
				return Err(Error::Invalid(
					"upgrade the logical Agent to change its private policy".into(),
				));
			}
			return Ok(Outcome::Settings(Some(
				super::super::repositories::bank_settings::configure(
					lease,
					&input.bank,
					&input.provider,
					*expected_revision,
					input.operation_id,
					policy.retention.max_model_operations,
				)
				.await?,
			)));
		}
		Action::Jobs { after } => {
			return Ok(Outcome::Jobs(
				super::memory_administration::jobs(lease, &input.bank, *after).await?,
			));
		}
		Action::Usage { after } => {
			return Ok(Outcome::Usage(
				super::memory_administration::usage(lease, &input.bank, *after).await?,
			));
		}
		Action::Inspect { after } => {
			return Ok(Outcome::Inspection(
				super::memory_administration::inspect(lease, &input.bank, *after, &policy).await?,
			));
		}
		Action::History {
			id,
			expected_revision,
			before,
		} => {
			return Ok(Outcome::History(
				super::memory_administration::history(
					lease,
					&input.bank,
					*id,
					*expected_revision,
					*before,
					&policy,
				)
				.await?,
			));
		}
		Action::Settings => {
			return Ok(Outcome::Settings(
				super::super::repositories::bank_settings::get(lease, &input.bank).await?,
			));
		}
		Action::Reindex {
			expected_index_revision,
		} => {
			return Ok(Outcome::Units(
				super::memory_administration::reindex(
					lease,
					&input,
					&policy,
					*expected_index_revision,
				)
				.await?,
			));
		}
		Action::Candidates => {
			return Ok(Outcome::Candidates(
				candidates::list(lease, &input.bank, &policy.bounds).await?,
			));
		}
		Action::Review {
			id,
			expected_revision,
			mutation,
		} => {
			if let Some(mutation) = mutation {
				check_mutation(mutation)?;
			}
			return Ok(Outcome::Reviewed(
				candidates::review(
					lease,
					&input.bank,
					input.operation_id,
					*id,
					*expected_revision,
					mutation.as_ref(),
					&policy.bounds,
				)
				.await?
				.map(Box::new),
			));
		}
		Action::Publish { source, mutation } => {
			check_mutation(mutation)?;
			return Ok(Outcome::Units(
				publications::publish(lease, source, mutation, &policy.bounds).await?,
			));
		}
		_ => {}
	}
	// Validate the complete canonical journal before resolving a model or its
	// origin allowance. The caller cannot substitute prose or an arbitrary Run.
	let learning = if let Action::Learn { run: proof } = &input.action {
		Some(
			super::super::repositories::learning::input(lease, &input.bank, proof, &policy.bounds)
				.await?,
		)
	} else {
		None
	};
	let model_run = match &input.action {
		Action::Learn {
			run: Evidence::Run { id, .. },
		} => Some(*id),
		_ => run,
	};
	let models = super::memory_models::Models::resolve(
		store,
		lease,
		input.provider.clone(),
		input.bank.clone(),
		policy.clone(),
		input.operation_id,
		digest,
		model_run,
	)
	.await?;
	let engine = aidash_application::memory::Engine {
		provider: &input.provider,
		policy: &policy,
		models: &models,
	};
	let mut authority = Scope {
		store,
		lease,
		models: &models,
		run,
	};
	match &input.action {
		Action::Recall { query } => Ok(Outcome::Recall(
			engine.recall(&mut authority, &input.bank, query).await?,
		)),
		Action::Reflect { query } => Ok(Outcome::Reflection(
			engine.reflect(&mut authority, &input.bank, query).await?,
		)),
		Action::Retain { text, evidence } => Ok(Outcome::Units(
			engine
				.retain(
					&mut authority,
					&input.bank,
					input.operation_id,
					text,
					evidence,
				)
				.await?,
		)),
		Action::Learn { run } => {
			let (text, evidence) = learning.expect("validated learning input");
			Ok(Outcome::Candidates(
				engine
					.learn(&mut authority, &input.bank, run, &text, &evidence)
					.await?,
			))
		}
		Action::Derive {
			mutation,
			kind,
			sources,
		} => {
			check_mutation(mutation)?;
			authority.current(&input.bank, sources).await?;
			let mut units = Vec::new();
			for source in sources {
				let Evidence::Unit { bank, id, revision } = source else {
					return Err(Error::Invalid(
						"derivation reads admitted units only".into(),
					));
				};
				let unit = units::load(authority.lease, *id, false)
					.await?
					.ok_or(Error::Forbidden)?;
				if bank != &input.bank || unit.bank != *bank || unit.revision != *revision {
					return Err(Error::Conflict("derived source changed".into()));
				}
				units.push(unit);
			}
			Ok(Outcome::Units(
				engine
					.maintain(&mut authority, mutation, *kind, &units)
					.await?,
			))
		}
		_ => unreachable!("non-model operations returned above"),
	}
}

pub(crate) fn request_id(run: Uuid, key: &str) -> Result<Uuid> {
	let digest =
		aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(&(run, key))?);
	let mut bytes = [0u8; 16];
	for (index, byte) in bytes.iter_mut().enumerate() {
		*byte = u8::from_str_radix(&digest[index * 2..index * 2 + 2], 16)
			.map_err(|_| Error::Invalid("memory request identity".into()))?;
	}
	Ok(Uuid::from_bytes(bytes))
}
pub(crate) async fn run_mutate(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &aidash_domain::Run,
	key: &str,
	changes: &[Change],
) -> Result<Vec<Unit>> {
	if run.home_node != store.node_id {
		return Err(Error::Forbidden);
	}
	let binding = super::super::repositories::bindings::load(&mut **lease.tx(), &run.metadata())
		.await?
		.ok_or(Error::Forbidden)?;
	let mutation = Mutation {
		operation_id: request_id(run.id, key)?,
		provider: binding.provider,
		bank: binding.bank,
		changes: changes.to_vec(),
	};
	repository::lock_workspace(lease, run.workspace_id, true).await?;
	scope(store, lease, &mutation.bank, "memory.write").await?;
	bank_provider(lease, &mutation.bank, &mutation.provider).await?;
	let policy = policy(lease, &mutation.provider).await?;
	repository::mutate(lease, &mutation, &policy.bounds).await
}
pub(crate) async fn run_recall(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &aidash_domain::Run,
	key: &str,
	query: RecallQuery,
	reflect: bool,
) -> Result<Outcome> {
	if run.home_node != store.node_id {
		return Err(Error::Forbidden);
	}
	let binding = super::super::repositories::bindings::load(&mut **lease.tx(), &run.metadata())
		.await?
		.ok_or(Error::Forbidden)?;
	operate_in(
		store,
		lease,
		Operation {
			operation_id: request_id(run.id, key)?,
			provider: binding.provider,
			bank: binding.bank,
			action: if reflect {
				Action::Reflect { query }
			} else {
				Action::Recall { query }
			},
		},
		Some(run.id),
	)
	.await
}

/// Role configuration comes from the immutable definition, never an installation overlay.
pub(crate) async fn definition(
	lease: &mut Lease<'_>,
	reference: &EntityRef,
	kind: &str,
) -> Result<crate::registry::Entry> {
	let entry = crate::apps::registry::models::Definition::read_in(
		&mut **lease.tx(),
		&reference.id,
		&reference.version,
	)
	.await?;
	if entry.kind != kind {
		return Err(Error::Invalid(format!("memory role requires {kind}")));
	}
	if let Some(access) = lease.access() {
		let permitted =
			crate::authorization::catalog::entry(access, reference, "registry.read").await?;
		if permitted.config != entry.config {
			return Err(Error::Conflict(
				"memory roles require immutable Registry definitions".into(),
			));
		}
	}
	Ok(entry)
}
pub(crate) async fn policy(lease: &mut Lease<'_>, provider: &EntityRef) -> Result<Policy> {
	let entry = definition(lease, provider, "memory").await?;
	let config: ProviderConfig = serde_json::from_value(entry.config)?;
	config.policy.validate()?;
	Ok(config.policy)
}
pub(crate) async fn scope(
	store: &Store,
	lease: &mut Lease<'_>,
	bank: &Bank,
	action: &str,
) -> Result<()> {
	bank.validate()?;
	if bank.home != store.node_id {
		return Err(Error::Forbidden);
	}
	let tenant: Option<String> = native::query_scalar(
		&Query::select()
			.column(Alias::new("tenant"))
			.from(Alias::new("authorization_workspaces"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(bank.workspace)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **lease.tx())
	.await?;
	if tenant.as_deref() != Some(bank.tenant.as_str()) {
		return Err(Error::Forbidden);
	}
	units::authorize(lease, bank, action).await
}
pub(crate) async fn bank_provider(
	lease: &mut Lease<'_>,
	bank: &Bank,
	provider: &EntityRef,
) -> Result<()> {
	if let Some(id) = bank.participant {
		let row = native::query(
			&Query::select()
				.columns(["agent_id", "agent_version"].map(Alias::new))
				.from(Alias::new("memory_participants"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **lease.tx())
		.await?;
		let agent = definition(
			lease,
			&EntityRef {
				id: row.try_get("agent_id")?,
				version: row.try_get("agent_version")?,
			},
			"agent",
		)
		.await?;
		let config: crate::registry::AgentConfig = serde_json::from_value(agent.config)?;
		if config.memory.as_ref() != Some(provider) {
			return Err(Error::Conflict(
				"memory provider differs from the participant's accepted Agent version".into(),
			));
		}
	}
	if let Some(settings) = super::super::repositories::bank_settings::get(lease, bank).await?
		&& settings.provider != *provider
	{
		return Err(Error::Conflict(
			"memory bank policy differs from the requested Registry version".into(),
		));
	}
	Ok(())
}
pub async fn mutate(store: &Store, actor: &Actor, mutation: Mutation) -> Result<Vec<Unit>> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		repository::lock_workspace(&mut lease, mutation.bank.workspace, true).await?;
		scope(store, &mut lease, &mutation.bank, "memory.write").await?;
		bank_provider(&mut lease, &mutation.bank, &mutation.provider).await?;
		let policy = policy(&mut lease, &mutation.provider).await?;
		mutation.validate(&policy)?;
		repository::mutate(&mut lease, &mutation, &policy.bounds).await
	}
	.await;
	lease.finish(result).await
}
pub async fn list(store: &Store, actor: &Actor, input: ReadBank) -> Result<Vec<Unit>> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		scope(store, &mut lease, &input.bank, "memory.read").await?;
		bank_provider(&mut lease, &input.bank, &input.provider).await?;
		let policy = policy(&mut lease, &input.provider).await?;
		repository::list(
			&mut lease,
			&input.bank,
			policy.bounds.max_units,
			policy.bounds.max_graph_visits,
		)
		.await
	}
	.await;
	lease.finish(result).await
}

/// An explicit version upgrade retains the same logical identity. Changing the
/// Agent definition ID is a clone and must create a fresh participant instead.
pub async fn upgrade_participant(
	store: &Store,
	actor: &Actor,
	bank: Bank,
	input: UpgradeParticipant,
) -> Result<Participant> {
	if input.expected_revision < 1 || input.expected_revision >= i64::MAX - 1 {
		return Err(Error::Invalid("invalid participant revision".into()));
	}
	let id = bank
		.participant
		.ok_or_else(|| Error::Invalid("shared memory has no Agent participant".into()))?;
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		repository::lock_workspace(&mut lease, bank.workspace, true).await?;
		scope(store, &mut lease, &bank, "memory.participant.manage").await?;
		let accepted = definition(&mut lease, &input.agent, "agent").await?;
		let configuration: crate::registry::AgentConfig = serde_json::from_value(accepted.config)?;
		if let Some(provider) = &configuration.memory {
			policy(&mut lease, provider).await?;
		}
		let row = native::query(
			&Query::select()
				.columns(["agent_id", "revision"].map(Alias::new))
				.from(Alias::new("memory_participants"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.lock(reinhardt::query::LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **lease.tx())
		.await?;
		if row.try_get::<String>("agent_id")? != input.agent.id
			|| row.try_get::<i64>("revision")? != input.expected_revision
		{
			return Err(Error::Conflict(
				"participant definition or revision changed".into(),
			));
		}
		let revision = input.expected_revision + 1;
		native::query(
			&Query::update()
				.table(Alias::new("memory_participants"))
				.value_expr(
					Alias::new("agent_version"),
					Expr::value(&input.agent.version),
				)
				.value_expr(Alias::new("revision"), Expr::value(revision))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		if let Some(provider) = &configuration.memory {
			let settings =
				super::super::repositories::bank_settings::get(&mut lease, &bank).await?;
			if settings.as_ref().is_none_or(|s| s.provider != *provider) {
				super::super::repositories::bank_settings::set(
					&mut lease,
					&bank,
					provider,
					settings.map_or(0, |s| s.revision),
				)
				.await?;
			}
		}
		Ok(Participant {
			id,
			bank,
			agent: input.agent,
			revision,
		})
	}
	.await;
	lease.finish(result).await
}

/// Home issues a fresh participant identity. Agent names, definition versions and
/// processes never select an existing bank or inherit another participant's authority.
pub async fn create_participant(
	store: &Store,
	actor: &Actor,
	workspace: Uuid,
	input: CreateParticipant,
) -> Result<Participant> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		repository::lock_workspace(&mut lease, workspace, true).await?;
		lease
			.workspace(workspace, "memory.participant.manage")
			.await?;
		definition(&mut lease, &input.agent, "agent").await?;
		let tenant: String = native::query_scalar(
			&Query::select()
				.column(Alias::new("tenant"))
				.from(Alias::new("authorization_workspaces"))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **lease.tx())
		.await?;
		let principal = lease.saved()?["subject"]
			.as_str()
			.ok_or(Error::Forbidden)?
			.to_owned();
		let id = Uuid::now_v7();
		let bank = Bank {
			home: store.node_id.clone(),
			tenant,
			workspace,
			participant: Some(id),
		};
		bank.validate()?;
		native::query(
			&Query::insert()
				.into_table(Alias::new("memory_participants"))
				.columns(
					[
						"id",
						"home",
						"tenant",
						"workspace_id",
						"principal",
						"agent_id",
						"agent_version",
						"revision",
						"deleted",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(id))
						.expr(Expr::value(&bank.home))
						.expr(Expr::value(&bank.tenant))
						.expr(Expr::value(workspace))
						.expr(Expr::value(principal))
						.expr(Expr::value(&input.agent.id))
						.expr(Expr::value(&input.agent.version))
						.expr(Expr::value(1_i64))
						.expr(Expr::value(false))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		repository::bank_id(&mut lease, &bank, true).await?;
		Ok(Participant {
			id,
			bank,
			agent: input.agent,
			revision: 1,
		})
	}
	.await;
	lease.finish(result).await
}

impl NativeMemory {
	pub async fn operate(
		&self,
		actor: Actor,
		workspace: Uuid,
		input: Operation,
	) -> Result<Outcome> {
		if input.bank.workspace != workspace {
			return Err(Error::Forbidden);
		}
		operate(&self.runtime.store, &actor, input).await
	}
	pub async fn upgrade(
		&self,
		actor: Actor,
		workspace: Uuid,
		participant: Uuid,
		input: UpgradeParticipant,
	) -> Result<Participant> {
		let mut lease = Lease::begin(&self.runtime.store, &actor).await?;
		let found = async {
			let row = native::query(
				&Query::select()
					.columns(["home", "tenant", "workspace_id"].map(Alias::new))
					.from(Alias::new("memory_participants"))
					.and_where(Expr::col("id").eq(Expr::value(participant)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **lease.tx())
			.await?
			.ok_or(Error::Forbidden)?;
			let bank = Bank {
				home: row.try_get("home")?,
				tenant: row.try_get("tenant")?,
				workspace: row.try_get("workspace_id")?,
				participant: Some(participant),
			};
			if bank.workspace != workspace {
				return Err(Error::Forbidden);
			}
			Ok(bank)
		}
		.await;
		let bank = lease.finish(found).await?;
		upgrade_participant(&self.runtime.store, &actor, bank, input).await
	}
	pub async fn mutate(
		&self,
		actor: Actor,
		workspace: Uuid,
		input: Mutation,
	) -> Result<Vec<Unit>> {
		if input.bank.workspace != workspace {
			return Err(Error::Forbidden);
		}
		mutate(&self.runtime.store, &actor, input).await
	}
	pub async fn list(&self, actor: Actor, workspace: Uuid, input: ReadBank) -> Result<Vec<Unit>> {
		if input.bank.workspace != workspace {
			return Err(Error::Forbidden);
		}
		list(&self.runtime.store, &actor, input).await
	}
	pub async fn participant(
		&self,
		actor: Actor,
		workspace: Uuid,
		input: CreateParticipant,
	) -> Result<Participant> {
		create_participant(&self.runtime.store, &actor, workspace, input).await
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignedParticipant {
	pub participant_id: Uuid,
	pub participant_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignmentChange {
	pub task_revision: i64,
	pub expected: Option<AssignedParticipant>,
	pub target: Option<AssignedParticipant>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskAssignment {
	pub task_revision: i64,
	pub participant: Option<AssignedParticipant>,
}

pub async fn task_assignment(
	store: &Store,
	actor: &Actor,
	workspace: Uuid,
	task: Uuid,
) -> Result<TaskAssignment> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		lease.workspace(workspace, "workspace.read").await?;
		let record: crate::domain::Task = crate::database::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(Alias::new("tasks"))
				.and_where(Expr::col("id").eq(Expr::value(task)))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.lock(reinhardt::query::LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?
		.ok_or(Error::Forbidden)?;
		if let Some(access) = lease.access() {
			let resource = access.task_resource(&record).await?;
			access.require(&resource, "task.read").await?;
		}
		let participant = native::query(
			&Query::select()
				.columns(["participant_id", "participant_revision"].map(Alias::new))
				.from(Alias::new("memory_task_participants"))
				.and_where(Expr::col("task_id").eq(Expr::value(task)))
				.lock(reinhardt::query::LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?
		.map(|row| -> Result<_> {
			Ok(AssignedParticipant {
				participant_id: row.try_get("participant_id")?,
				participant_revision: row.try_get("participant_revision")?,
			})
		})
		.transpose()?;
		if let Some(assigned) = &participant {
			let row = native::query(
				&Query::select()
					.columns(["home", "tenant", "workspace_id"].map(Alias::new))
					.from(Alias::new("memory_participants"))
					.and_where(Expr::col("id").eq(Expr::value(assigned.participant_id)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **lease.tx())
			.await?;
			scope(
				store,
				&mut lease,
				&Bank {
					home: row.try_get("home")?,
					tenant: row.try_get("tenant")?,
					workspace: row.try_get("workspace_id")?,
					participant: Some(assigned.participant_id),
				},
				"memory.read",
			)
			.await?;
		}
		Ok(TaskAssignment {
			task_revision: record.revision,
			participant,
		})
	}
	.await;
	lease.finish(result).await
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ParticipantPage {
	pub items: Vec<Participant>,
	pub next: Option<Uuid>,
}

pub async fn participants(
	store: &Store,
	actor: &Actor,
	workspace: Uuid,
	after: Option<Uuid>,
) -> Result<ParticipantPage> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		lease.workspace(workspace, "workspace.read").await?;
		let mut query = Query::select();
		query
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("memory_participants"))
			.and_where(Expr::col("home").eq(store.node_id.as_str()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(Expr::col("deleted").eq(false))
			.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
			.limit(101);
		if let Some(after) = after {
			query.and_where(Expr::col("id").gt(Expr::value(after)));
		}
		let rows = native::query(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut **lease.tx())
			.await?;
		let next = if rows.len() > 100 {
			Some(rows[99].try_get("id")?)
		} else {
			None
		};
		let mut items = vec![];
		for row in rows.into_iter().take(100) {
			let id = row.try_get("id")?;
			let bank = Bank {
				home: row.try_get("home")?,
				tenant: row.try_get("tenant")?,
				workspace,
				participant: Some(id),
			};
			match scope(store, &mut lease, &bank, "memory.read").await {
				Err(Error::Forbidden) => continue,
				result => result?,
			}
			items.push(Participant {
				id,
				bank,
				agent: EntityRef {
					id: row.try_get("agent_id")?,
					version: row.try_get("agent_version")?,
				},
				revision: row.try_get("revision")?,
			});
		}
		Ok(ParticipantPage { items, next })
	}
	.await;
	lease.finish(result).await
}

pub async fn assign_participant(
	store: &Store,
	actor: &Actor,
	workspace: Uuid,
	task: Uuid,
	input: AssignmentChange,
) -> Result<Option<AssignedParticipant>> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		super::super::repositories::candidates::human(&mut lease)?;
		repository::lock_workspace(&mut lease, workspace, true).await?;
		lease
			.workspace(workspace, "memory.participant.manage")
			.await?;
		let task_row: crate::domain::Task = crate::database::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(Alias::new("tasks"))
				.and_where(Expr::col("id").eq(Expr::value(task)))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.lock(reinhardt::query::LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?
		.ok_or(Error::Forbidden)?;
		if task_row.revision != input.task_revision
			|| task_row.status != aidash_domain::TaskStatus::Open
		{
			return Err(Error::Conflict(
				"participant assignment requires the observed open Task revision".into(),
			));
		}
		if let Some(access) = lease.access() {
			let resource = access.task_resource(&task_row).await?;
			access.require(&resource, "task.claim").await?;
		}
		let prior = native::query(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(Alias::new("memory_task_participants"))
				.and_where(Expr::col("task_id").eq(Expr::value(task)))
				.lock(reinhardt::query::LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?
		.map(|row| -> Result<_> {
			Ok(AssignedParticipant {
				participant_id: row.try_get("participant_id")?,
				participant_revision: row.try_get("participant_revision")?,
			})
		})
		.transpose()?;
		if prior != input.expected && prior != input.target {
			return Err(Error::Conflict(
				"observed participant assignment changed".into(),
			));
		}
		if let Some(target) = &input.target {
			let row = native::query(
				&Query::select()
					.column(reinhardt::query::ColumnRef::Asterisk)
					.from(Alias::new("memory_participants"))
					.and_where(Expr::col("id").eq(Expr::value(target.participant_id)))
					.lock(reinhardt::query::LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **lease.tx())
			.await?
			.ok_or(Error::Forbidden)?;
			let bank = Bank {
				home: row.try_get("home")?,
				tenant: row.try_get("tenant")?,
				workspace: row.try_get("workspace_id")?,
				participant: Some(target.participant_id),
			};
			if bank.workspace != workspace {
				return Err(Error::Forbidden);
			}
			scope(store, &mut lease, &bank, "memory.participant.use").await?;
			if row.try_get::<i64>("revision")? != target.participant_revision {
				return Err(Error::Conflict(
					"logical participant revision changed".into(),
				));
			}
		}
		if prior == input.target {
			return Ok(input.target);
		}
		native::query(
			&Query::delete()
				.from_table(Alias::new("memory_task_participants"))
				.and_where(Expr::col("task_id").eq(Expr::value(task)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		if let Some(target) = &input.target {
			native::query(
				&Query::insert()
					.into_table(Alias::new("memory_task_participants"))
					.columns(["task_id", "participant_id", "participant_revision"].map(Alias::new))
					.from_subquery(
						Query::select()
							.expr(Expr::value(task))
							.expr(Expr::value(target.participant_id))
							.expr(Expr::value(target.participant_revision))
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
		}
		Ok(input.target)
	}
	.await;
	lease.finish(result).await
}
impl NativeMemory {
	pub async fn assignment(
		&self,
		actor: Actor,
		workspace: Uuid,
		task: Uuid,
	) -> Result<TaskAssignment> {
		task_assignment(&self.runtime.store, &actor, workspace, task).await
	}
	pub async fn participants(
		&self,
		actor: Actor,
		workspace: Uuid,
		after: Option<Uuid>,
	) -> Result<ParticipantPage> {
		participants(&self.runtime.store, &actor, workspace, after).await
	}
	pub async fn assign(
		&self,
		actor: Actor,
		workspace: Uuid,
		task: Uuid,
		input: AssignmentChange,
	) -> Result<Option<AssignedParticipant>> {
		assign_participant(&self.runtime.store, &actor, workspace, task, input).await
	}
}
