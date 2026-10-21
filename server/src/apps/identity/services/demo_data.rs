//! `aidash demo-data --from <structure.json> [--people <people.json>]` loads
//! Demo data into a Deployment (ADR 0079, ADR 0080). It refuses to run unless
//! `[demo_data] enabled` is on, writes only through Reinhardt in the
//! `demo-data` Deployment Scope, and writes the digest of its inputs last, so
//! a rerun with the same inputs changes nothing.
use crate::apps::identity::models::{DemoDataLoad, database_time};
use crate::config::Config;
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use reinhardt::db::orm::Model;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod approvals;
mod input;
mod people;
mod registry;
mod subjects;
mod tenants;
pub use input::{Inputs, People, Person, Role, Structure};

fn scope() -> TenancyScope {
	TenancyScope::Deployment(DeploymentScopeReason::DemoData)
}

/// What a run did.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
	Unchanged,
	Loaded,
}

async fn run(structure: &Path, people: Option<&Path>) -> Result<Outcome> {
	let settings = crate::config::startup::load_settings()?;
	// The guard comes first: a Deployment that has not opted in is never
	// touched, not even to look at its schema.
	if !settings.demo_data.enabled {
		return Err(Error::Invalid(
			"demo-data is off: set [demo_data] enabled = true (AIDASH_DEMO_DATA_ENABLED); nothing was written"
				.into(),
		));
	}
	let inputs = read(structure, people).await?;
	// Loading never changes the schema; `migrate` runs first as its owner.
	crate::bootstrap::require_migrated(&settings).await?;
	let config = Config::from_settings(&settings)?;
	// The migration Job runs this command with the migration owner's login,
	// which the server's own connection refuses. `FORCE ROW LEVEL SECURITY`
	// binds the owner, and every write below opens a tenancy scope.
	let database = crate::database::runtime::connect_maintenance(&config.database_url, 8).await?;
	// The sign-in policy decides whether a person's existing External
	// Identity is mapped at once (ADR 0079).
	let pool = Pool::from(database.into_postgres().ok_or(Error::Forbidden)?)
		.with_dashboard_policy(config.dashboard_policy());
	let outcome = if loaded(&pool, &inputs.digest).await? {
		Outcome::Unchanged
	} else {
		load(&pool, &config, &inputs).await?;
		record(&pool, &inputs.digest).await?;
		Outcome::Loaded
	};
	pool.close().await;
	Ok(outcome)
}

async fn read(structure: &Path, people: Option<&Path>) -> Result<Inputs> {
	let file = async |path: &Path| {
		tokio::fs::read_to_string(path)
			.await
			.map_err(|error| Error::Invalid(format!("{}: {error}", path.display())))
	};
	let structure = file(structure).await?;
	let people = match people {
		Some(path) => Some(file(path).await?),
		None => None,
	};
	Inputs::parse(&structure, people.as_deref())
}

/// Whether these exact inputs were loaded to completion before.
async fn loaded(pool: &Pool, digest: &str) -> Result<bool> {
	pool.atomic(&scope(), async |tx| {
		Ok(DemoDataLoad::objects()
			.filter(DemoDataLoad::field_digest().eq(digest.to_owned()))
			.exists_with_db(tx)
			.await?)
	})
	.await
}

/// Loads the structure and the people. Each step is idempotent and nothing is
/// ever deleted. `record` stays the last write of a run.
///
/// A Tenant that a reviewer deleted is left deleted: it gets nothing more,
/// and nobody is mapped into it.
async fn load(pool: &Pool, config: &Config, inputs: &Inputs) -> Result<()> {
	let node = &config.node_id;
	let named = tenants::ensure(pool, node, &inputs.structure).await?;
	let mut live = BTreeMap::new();
	for tenant in &inputs.structure.tenants {
		let name = &named[&tenant.key];
		let wanted = wanted_subjects(tenant, inputs);
		if !subjects::add(pool, name, &wanted).await? {
			tracing::warn!(key = %tenant.key, "demo-data skips a deleted Tenant");
			continue;
		}
		registry::register(pool, node, &tenant.key, name, &tenant.registry).await?;
		approvals::approve(pool, node, &tenant.key, name, &tenant.approvals).await?;
		live.insert(tenant.key.clone(), name.clone());
	}
	let people: Vec<Person> = inputs
		.people
		.iter()
		.flat_map(|people| people.people.iter())
		.filter(|person| live.contains_key(&person.tenant))
		.cloned()
		.collect();
	people::load(pool, &live, &people).await
}

/// The user subjects one Tenant should hold: the structure's fictional
/// subjects, then the subject of every person that names the Tenant, in the
/// group their role gives. A subject the structure lists keeps its groups.
fn wanted_subjects(tenant: &input::StructureTenant, inputs: &Inputs) -> Vec<subjects::Wanted> {
	let mut wanted: Vec<subjects::Wanted> = tenant
		.subjects
		.iter()
		.map(|subject| (subject.id.clone(), subject.groups.clone()))
		.collect();
	for person in inputs.people_of(&tenant.key) {
		if !wanted.iter().any(|(id, _)| *id == person.subject) {
			wanted.push((
				person.subject.clone(),
				BTreeSet::from([person.role.group().to_owned()]),
			));
		}
	}
	wanted
}

/// Records that the inputs with `digest` were loaded to completion. It is its
/// own transaction, after every step: a run that stopped part-way leaves no
/// row, so the next run repeats the steps.
async fn record(pool: &Pool, digest: &str) -> Result<()> {
	pool.atomic(&scope(), async |tx| {
		let exists = DemoDataLoad::objects()
			.filter(DemoDataLoad::field_digest().eq(digest.to_owned()))
			.exists_with_db(tx)
			.await?;
		if !exists {
			let now = database_time(tx).await?;
			let load = DemoDataLoad::build()
				.digest(digest.to_owned())
				.loaded_at(now)
				.finish();
			DemoDataLoad::objects().create_with_conn(tx, &load).await?;
		}
		Ok(())
	})
	.await
}

pub struct Command;
#[async_trait::async_trait]
impl reinhardt::commands::CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("demo-data")
			.about("Load Demo data; refuses to run unless [demo_data] enabled is on")
			.arg(
				clap::Arg::new("from")
					.long("from")
					.required(true)
					.value_name("STRUCTURE_JSON")
					.value_parser(clap::value_parser!(PathBuf))
					.help("The structure of the fictional Tenants"),
			)
			.arg(
				clap::Arg::new("people")
					.long("people")
					.value_name("PEOPLE_JSON")
					.value_parser(clap::value_parser!(PathBuf))
					.help("Who may claim which fictional subject, by verified email"),
			)
	}
	fn requirements(
		&self,
		_: &clap::ArgMatches,
	) -> Vec<reinhardt::commands::CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(
		&self,
		matches: &clap::ArgMatches,
		_: &reinhardt::commands::CapabilityContext,
	) -> reinhardt::commands::CommandResult<()> {
		let result: Result<()> = async {
			let structure = matches
				.get_one::<PathBuf>("from")
				.ok_or_else(|| Error::Invalid("demo-data requires --from".into()))?;
			let people = matches.get_one::<PathBuf>("people");
			match run(structure, people.map(PathBuf::as_path)).await? {
				Outcome::Unchanged => println!("Demo data unchanged"),
				Outcome::Loaded => println!("Demo data loaded"),
			}
			Ok(())
		}
		.await;
		result.map_err(|error| reinhardt::commands::CommandError::ExecutionError(error.to_string()))
	}
}
