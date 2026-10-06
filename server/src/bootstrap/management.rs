//! Shared Reinhardt command driver for management and deployment entry points.
use crate::config::settings::{ProjectSettings, get_scoped_settings, get_settings};
#[cfg(feature = "commands-shell")]
use crate::config::shell::get_shell_config;
#[cfg(not(feature = "commands-shell"))]
use reinhardt::commands::execute_from_command_line_with_capabilities;
#[cfg(feature = "commands-shell")]
use reinhardt::commands::execute_from_command_line_with_capabilities_and_shell;
use reinhardt::commands::{CapabilityProvider, CargoCheckContext, command_error_exit_code};
use reinhardt::conf::settings::PendingSettings;
use reinhardt::conf::settings::builder::BuildError;
use reinhardt::conf::settings::scoped::ScopedSettings;
use std::path::PathBuf;
use std::process;

struct ProjectProvider;

impl CapabilityProvider for ProjectProvider {
	type Settings = ProjectSettings;

	fn scoped_settings(&self) -> Result<ScopedSettings, BuildError> {
		get_scoped_settings()
	}

	fn full_settings(&self) -> Result<PendingSettings<ProjectSettings>, BuildError> {
		get_settings()
	}
}

pub async fn run(compatibility: bool) {
	let cargo_context = CargoCheckContext::from_launcher(
		PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
		Some(env!("CARGO_PKG_NAME").to_owned()),
		Some("manage".to_owned()),
	);

	if let Some(result) = crate::bootstrap::migrations::manage(&ProjectProvider).await {
		if let Err(error) = result {
			eprintln!("Error: {error}");
			process::exit(command_error_exit_code(&error));
		}
		return;
	}

	if compatibility && std::env::args_os().len() == 1 {
		let registry = super::management_commands();
		let command = registry
			.get_capability("serve")
			.expect("serve is registered");
		let matches = command.cli().get_matches_from(["serve"]);
		let context = reinhardt::commands::CapabilityContext::prepare(
			"serve",
			&command.requirements(&matches),
			&ProjectProvider,
		)
		.await;
		let result = match context {
			Ok(context) => command.execute(&matches, &context).await,
			Err(error) => Err(error),
		};
		if let Err(error) = result {
			eprintln!("Error: {error}");
			process::exit(command_error_exit_code(&error));
		}
		if let Err(error) = crate::config::startup::shutdown().await {
			eprintln!("Error: {error}");
			process::exit(1);
		}
		return;
	}

	// The command is selected before either settings provider runs.
	// Static commands resolve selected asset inputs; runtime commands
	// retain the full composed-settings validation path.
	#[cfg(feature = "commands-shell")]
	let result = execute_from_command_line_with_capabilities_and_shell(
		crate::bootstrap::management_commands(),
		ProjectProvider,
		Some(cargo_context),
		get_shell_config(),
	)
	.await;
	#[cfg(not(feature = "commands-shell"))]
	let result = execute_from_command_line_with_capabilities(
		crate::bootstrap::management_commands(),
		ProjectProvider,
		Some(cargo_context),
	)
	.await;

	let drained = crate::config::startup::shutdown().await;
	if let Err(error) = drained {
		eprintln!("Error: {error}");
		process::exit(1);
	}
	if let Err(e) = result {
		let exit_code = command_error_exit_code(e.as_ref());
		eprintln!("Error: {}", e);
		process::exit(exit_code);
	}
}
