//! Generate the one-time ORM state accompanying the preserved SQL baseline.
//! Output goes to a new directory for review; no database is contacted.
use reinhardt::db::migrations::{
	FilesystemRepository, FilesystemSource, Migration, MigrationAutodetector, MigrationGraph,
	MigrationKey, MigrationRepository, MigrationSource, ProjectState,
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let output = PathBuf::from(
		std::env::args_os()
			.nth(1)
			.ok_or("a new output directory is required")?,
	);
	std::fs::create_dir(&output)?;
	let _routes = aidash_server::routes();
	let models = ProjectState::try_from_global_registry()?;
	let baseline =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await?
			.into_iter()
			.filter(|migration| !migration.state_only)
			.collect::<Vec<_>>();
	let mut graph = MigrationGraph::new();
	for migration in &baseline {
		graph.add_migration(
			MigrationKey::new(&migration.app_label, &migration.name),
			migration
				.dependencies
				.iter()
				.map(|(app, name)| MigrationKey::new(app, name))
				.collect(),
		);
	}
	let order = graph.topological_sort()?;
	let tail = order.last().ok_or("the SQL baseline is missing")?;
	let mut previous = (tail.app_label.clone(), tail.name.clone());
	let mut repository = FilesystemRepository::new(output);
	let apps = aidash_server::config::apps::APP_LABELS;
	for app in apps {
		let operations = MigrationAutodetector::new(ProjectState::new(), models.filter_by_app(app))
			.try_generate_operations()?;
		let mut migration = Migration::new("0007_model_state", app)
			.state_only(true)
			.add_dependency(previous.0.clone(), previous.1.clone());
		for operation in operations {
			migration = migration.add_operation(operation);
		}
		repository.save(&migration).await?;
		previous = (app.to_owned(), migration.name);
	}
	Ok(())
}
