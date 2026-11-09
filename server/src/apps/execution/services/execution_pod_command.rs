//! Operator diagnostic for starting a Scratch Space's Execution Pod.
use crate::apps::{
	execution::repositories::scope_egress::ScratchAuthority, workspaces::repositories::scratch,
};
use crate::{
	Error, Result,
	config::{Config, ScratchOperatorPolicy},
	store::Store,
};
use aidash_application::{
	egress::ScopeAuthority,
	ports::capabilities::execution_pod::{
		EgressCredential, ExecutionPodError, ExecutionPodErrorCode, ExecutionPodRuntime,
		ExecutionPodSettings,
	},
	tenancy::TenancyScope,
};
use aidash_domain::scratch::{ScratchSpace, ScratchState};
use reinhardt::commands::{
	CapabilityCommand, CapabilityContext, CapabilityRequirement, CommandError, CommandResult,
};
use uuid::Uuid;

pub struct Command;
#[async_trait::async_trait]
impl CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("execution-pod")
			.about("Inspect an Execution Pod")
			.subcommand_required(true)
			.arg_required_else_help(true)
			.subcommand(
				clap::Command::new("ensure")
					.about("Ensure the Scratch Space's Execution Pod is ready")
					.arg(
						clap::Arg::new("space")
							.long("space")
							.required(true)
							.value_parser(clap::value_parser!(Uuid)),
					),
			)
	}
	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(
		&self,
		matches: &clap::ArgMatches,
		_: &CapabilityContext,
	) -> CommandResult<()> {
		let result: Result<()> = async {
			let (_, args) = matches
				.subcommand()
				.ok_or_else(|| Error::Invalid("execution-pod requires ensure".into()))?;
			let space = *args
				.get_one::<Uuid>("space")
				.ok_or_else(|| Error::Invalid("ensure requires --space".into()))?;
			let settings = crate::config::startup::load_settings()?;
			crate::bootstrap::require_migrated(&settings).await?;
			let config = Config::from_settings(&settings)?;
			let connection =
				crate::database::runtime::connect_command(&config.database_url, 4).await?;
			let pool = connection.into_postgres().ok_or(Error::Forbidden)?;
			let store = Store::from_pool(pool, config.deployment.clone()).await?;
			let result = run(&store, &config.scratch_policy, space).await;
			store.pool.close().await;
			store.control_pool.close().await;
			store.restoration_pool.close().await;
			println!("{}", result?);
			Ok(())
		}
		.await;
		result.map_err(|error| CommandError::ExecutionError(error.to_string()))
	}
}

async fn run(store: &Store, policy: &ScratchOperatorPolicy, id: Uuid) -> Result<&'static str> {
	let tenant = scratch::tenant_by_id(&store.pool, id).await?;
	let mut tx = store.pool.begin(&TenancyScope::tenant(&tenant)?).await?;
	let space = scratch::read(&mut *tx, &tenant, id)
		.await?
		.ok_or_else(|| Error::NotFound("Scratch Space".into()))?;
	tx.commit().await?;
	let authority = ScratchAuthority::new(store.clone(), policy.clone());
	let credential = authority.credential(&tenant, id)?;
	// Recheck current state and Tenant/Node deletion using the gateway's authority.
	authority.authenticate(&credential).await?;
	let runner = crate::bootstrap::operation_runner(store)?;
	let image = &store
		.capabilities
		.0
		.runner
		.as_ref()
		.ok_or_else(|| Error::Invalid("Runner unavailable".into()))?
		.image;
	let settings = pod_settings(&space, &tenant, policy, image, credential)?;
	ensure_output(&runner, &settings).await
}

fn pod_settings(
	space: &ScratchSpace,
	tenant: &str,
	policy: &ScratchOperatorPolicy,
	default_image: &str,
	credential: String,
) -> Result<ExecutionPodSettings> {
	if space.state != ScratchState::Active {
		return Err(Error::Forbidden);
	}
	let (ceilings, hosts) = scratch::effective_policy(policy, tenant);
	let settings = space.settings.effective_for(&ceilings, &hosts);
	if settings.docker_enabled && !ceilings.docker_allowed {
		return Err(Error::Forbidden);
	}
	if let Some(image) = &settings.execution_image
		&& !policy
			.tenant_images
			.get(tenant)
			.is_some_and(|images| images.contains(image))
	{
		return Err(Error::Forbidden);
	}
	// The Runner protocol accepts whole cores only. Never exceed the Space's limit.
	let cpu = settings.cpu_millis / 1000;
	if cpu == 0 {
		return Err(Error::Invalid(
			"Execution Pod requires at least 1000 millicores".into(),
		));
	}
	Ok(ExecutionPodSettings {
		scratch_space_id: space.id.to_string(),
		image: settings
			.execution_image
			.unwrap_or_else(|| default_image.to_owned()),
		cpu,
		memory_bytes: settings.memory_bytes,
		capacity_bytes: settings.capacity_bytes,
		docker_enabled: settings.docker_enabled,
		tenant_id: Some(tenant.to_owned()),
		egress_credential: Some(EgressCredential(credential)),
	})
}

async fn ensure_output(
	runner: &impl ExecutionPodRuntime,
	settings: &ExecutionPodSettings,
) -> Result<&'static str> {
	// A Runner response or error can contain arbitrary text, including credentials.
	// A successful ensure means the Pod is ready; print no Runner-supplied text.
	runner.ensure(settings).await.map_err(|error| {
		let code = match error {
			ExecutionPodError::Remote {
				code: ExecutionPodErrorCode::Unknown(_),
				..
			} => "runner_error".to_owned(),
			ExecutionPodError::Transport(_) => "runner_unavailable".to_owned(),
			ExecutionPodError::Remote { code, .. } => code.to_string(),
		};
		Error::External(format!("Execution Pod ensure failed: {code}"))
	})?;
	Ok("Execution Pod: ready")
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_application::ports::Credentials;
	use aidash_integrations::runner::RunnerHttp;
	use serde_json::{Value, json};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	use wiremock::{
		Mock, MockServer, Request, ResponseTemplate,
		matchers::{method, path},
	};

	struct Keys;
	impl Credentials for Keys {
		fn resolve(&self, _: &str) -> aidash_application::Result<String> {
			Ok("runner-key".into())
		}
	}

	#[test]
	fn settings_use_current_limits_and_never_round_cpu_above_the_space_limit() {
		let now = chrono::Utc::now();
		let id = Uuid::new_v4();
		let parsed = Command
			.cli()
			.try_get_matches_from(["execution-pod", "ensure", "--space", &id.to_string()])
			.unwrap();
		let id = *parsed
			.subcommand_matches("ensure")
			.unwrap()
			.get_one::<Uuid>("space")
			.unwrap();
		let mut space = ScratchSpace {
			id,
			node: aidash_domain::nodes::NodeName::derived("test"),
			name: "test".into(),
			creator: "alice".into(),
			members: Default::default(),
			state: ScratchState::Active,
			revision: 0,
			wipe_generation: 0,
			settings: Default::default(),
			created_at: now,
			last_used_at: now,
		};
		space.settings.cpu_millis = 2500;
		space.settings.memory_bytes = 99;
		let policy = ScratchOperatorPolicy::default();
		let settings = pod_settings(
			&space,
			"acme",
			&policy,
			"default-image",
			"secret-canary".into(),
		)
		.unwrap();
		assert_eq!(settings.scratch_space_id, space.id.to_string());
		assert_eq!(settings.tenant_id.as_deref(), Some("acme"));
		assert_eq!(settings.cpu, 2);
		assert_eq!(settings.memory_bytes, 99);
		assert_eq!(settings.image, "default-image");
		assert_eq!(settings.egress_credential.unwrap().0, "secret-canary");
		space.settings.cpu_millis = 999;
		assert!(
			pod_settings(
				&space,
				"acme",
				&policy,
				"default-image",
				"secret-canary".into()
			)
			.is_err()
		);
	}

	#[tokio::test]
	async fn runner_receives_credential_but_output_and_error_do_not_echo_it() {
		let parsed = Command
			.cli()
			.try_get_matches_from([
				"execution-pod",
				"ensure",
				"--space",
				"00000000-0000-0000-0000-000000000000",
			])
			.unwrap();
		let id = parsed
			.subcommand_matches("ensure")
			.unwrap()
			.get_one::<Uuid>("space")
			.unwrap();
		let calls = Arc::new(AtomicUsize::new(0));
		let counter = calls.clone();
		let server = MockServer::start().await;
		Mock::given(method("POST"))
			.and(path("/v2/pods/ensure"))
			.respond_with(move |request: &Request| {
				let body: Value = request.body_json().unwrap();
				assert_eq!(body["egress_credential"], "secret-canary");
				match counter.fetch_add(1, Ordering::SeqCst) {
					0 => ResponseTemplate::new(200).set_body_json(
						json!({"protocol":"aidash-runner/2", "pod":"secret-canary", "claim":"secret-canary"}),
					),
					1 => ResponseTemplate::new(400).set_body_json(
						json!({"code":"invalid_request", "message":"secret-canary"}),
					),
					_ => ResponseTemplate::new(400)
						.set_body_json(json!({"code":"secret-canary", "message":"secret-canary"})),
				}
			})
			.expect(3)
			.mount(&server)
			.await;
		let runner = RunnerHttp {
			endpoint: server.uri(),
			credential_env: "test".into(),
			credentials: Arc::new(Keys),
		};
		let settings = ExecutionPodSettings {
			scratch_space_id: id.to_string(),
			image: "image".into(),
			cpu: 2,
			memory_bytes: 1024,
			capacity_bytes: 2048,
			docker_enabled: false,
			tenant_id: Some("acme".into()),
			egress_credential: Some(EgressCredential("secret-canary".into())),
		};
		let output = ensure_output(&runner, &settings).await.unwrap();
		assert_eq!(output, "Execution Pod: ready");
		assert!(!output.contains("secret-canary"));
		let error = ensure_output(&runner, &settings)
			.await
			.unwrap_err()
			.to_string();
		assert!(!error.contains("secret-canary"));
		assert!(error.contains("invalid_request"));
		let unknown = ensure_output(&runner, &settings)
			.await
			.unwrap_err()
			.to_string();
		assert!(!unknown.contains("secret-canary"));
		assert_eq!(calls.load(Ordering::SeqCst), 3);
	}
}
