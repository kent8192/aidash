use super::*;
use crate::duties::scripts::{Language, MAX_SOURCE_BYTES};
use rstest::rstest;
use serde_json::{Value, json};

fn now() -> DateTime<Utc> {
	DateTime::parse_from_rfc3339("2026-11-20T00:00:00Z")
		.unwrap()
		.with_timezone(&Utc)
}

fn script(source: &str) -> Script {
	Script {
		language: Language::Python,
		source: source.into(),
	}
}

fn definition() -> Definition {
	Definition {
		task_model: None,
		title: "  GitHub CI duty ".into(),
		mission: "Fix failing CI runs on main.\nKeep the build green.".into(),
		procedure: "1. Read the failed job.\n2. Open a pull request.".into(),
		agent: EntityRef {
			id: "ci-fixer".into(),
			version: "1.2.0".into(),
		},
		trigger: Some(Trigger::Cron {
			expression: "0 9 * * 1-5".into(),
			time_zone: "Asia/Tokyo".into(),
		}),
		approval_mode: Some(crate::approval_rules::ApprovalMode::Auto),
		max_lateness_seconds: 120,
		pause_after_failures: 0,
		router: Some(script("print('{\"action\":\"task\"}')")),
		watcher: Some(Watcher {
			script: script("print('[]')"),
			every_seconds: 300,
		}),
	}
}

fn notes() -> Vec<TriggerNote> {
	vec![
		TriggerNote {
			kind: Slot::Time,
			instruction: "Weekday mornings in your time zone.".into(),
		},
		TriggerNote {
			kind: Slot::Webhook,
			instruction: "Add the URL as a workflow_run webhook in GitHub.".into(),
		},
	]
}

fn template() -> Template {
	Template::from_duty(
		&definition(),
		&[TriggerKind::Webhook, TriggerKind::Watcher],
		&notes(),
		true,
	)
	.unwrap()
}

#[rstest]
fn a_template_of_a_duty_keeps_its_work_and_leaves_its_people_and_ids_behind() {
	// Arrange
	let duty = definition();
	// Act
	let template = Template::from_duty(
		&duty,
		&[
			TriggerKind::Webhook,
			TriggerKind::Webhook,
			TriggerKind::Watcher,
			TriggerKind::DutyCompleted {
				source_duty_id: Uuid::from_u128(7),
			},
		],
		&notes(),
		true,
	)
	.unwrap();
	// Assert: the work goes, one trigger per kind, the Watcher's events none.
	assert_eq!(template.title, "GitHub CI duty");
	assert_eq!(template.mission, duty.mission);
	assert_eq!(template.procedure, duty.procedure);
	assert_eq!(template.router, duty.router);
	assert_eq!(template.watcher, duty.watcher);
	assert_eq!(
		template
			.triggers
			.iter()
			.map(ExpectedTrigger::slot)
			.collect::<Vec<_>>(),
		vec![Slot::Time, Slot::Webhook, Slot::DutyCompleted]
	);
	assert_eq!(
		template.agent,
		Some(SuggestedAgent {
			id: "ci-fixer".into(),
			version: "1.2.0".into(),
		})
	);
	// Assert: no source Duty id, Approval Mode or other person's setting.
	let text = serde_json::to_string(&template).unwrap();
	assert!(!text.contains(&Uuid::from_u128(7).to_string()), "{text}");
	assert!(!text.contains("approval_mode"), "{text}");
	assert!(!text.contains("pause_after_failures"), "{text}");
}

#[rstest]
fn a_template_suggests_no_agent_unless_asked() {
	let template = Template::from_duty(&definition(), &[], &notes()[..1], false).unwrap();
	assert_eq!(template.agent, None);
}

#[rstest]
#[case(vec![TriggerNote { kind: Slot::NodeEvent, instruction: "x".into() }], "no node_event trigger")]
#[case(
	vec![
		TriggerNote { kind: Slot::Time, instruction: "a".into() },
		TriggerNote { kind: Slot::Time, instruction: "b".into() },
	],
	"two instructions"
)]
fn a_note_must_match_one_trigger_of_the_duty(
	#[case] notes: Vec<TriggerNote>,
	#[case] reason: &str,
) {
	let error = Template::from_duty(&definition(), &[TriggerKind::Webhook], &notes, true)
		.unwrap_err()
		.to_string();
	assert!(error.contains(reason), "{error}");
}

#[rstest]
fn a_published_template_needs_an_instruction_for_every_trigger() {
	// Arrange: the webhook has no instruction.
	let template =
		Template::from_duty(&definition(), &[TriggerKind::Webhook], &notes()[..1], true).unwrap();
	// Act
	let error = template.validate(now()).unwrap_err().to_string();
	// Assert
	assert!(
		error.contains("webhook trigger needs an instruction"),
		"{error}"
	);
	assert!(self::template().validate(now()).is_ok());
}

#[rstest]
#[case::subject_credential("token = 'aidash_subject_acme_0123456789abcdefghijklmnop'", true)]
#[case::github(
	"curl -H 'Authorization: token ghp_0123456789abcdefABCDEF0123456789abcd'",
	true
)]
#[case::github_fine_grained("GITHUB=github_pat_11ABCDEFG0123456789_abcdefghijklmnop", true)]
#[case::aws("AKIAIOSFODNN7EXAMPLE", true)]
// Assembled at run time so the source holds no token-shaped literal (push protection).
#[case::slack(concat!("xox", "b-123456789012-1234567890123-abcdefghijklmnop"), true)]
#[case::anthropic("sk-ant-api03-abcdefghijklmnopqrstuvwxyz", true)]
#[case::private_key("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n", true)]
#[case::prose("Create a ghp_ token and store it as a Tenant Secret.", false)]
#[case::short_tail("AKIA keys are AWS access key ids", false)]
#[case::inside_a_word("the value is notghp_0123456789abcdefABCDEF here", false)]
#[case::environment("token = os.environ['GITHUB_TOKEN']", false)]
fn a_credential_written_into_text_looks_secret(#[case] text: &str, #[case] secret: bool) {
	assert_eq!(secret_like(text), secret, "{text}");
}

#[rstest]
#[case::mission(|t: &mut Template| t.mission.push_str(" ghp_0123456789abcdefABCDEF0123456789abcd"), "mission")]
#[case::procedure(|t: &mut Template| t.procedure = "use AKIAIOSFODNN7EXAMPLE".into(), "procedure")]
#[case::router(|t: &mut Template| t.router = Some(script("KEY='sk-or-v1-0123456789abcdef0123456789'")), "Router")]
#[case::watcher(
	|t: &mut Template| t.watcher.as_mut().unwrap().script = script("x='xoxp-1234567890-abcdefghijklmnop'"),
	"Watcher"
)]
#[case::instruction(
	|t: &mut Template| t.triggers[1] = ExpectedTrigger::Webhook { instruction: "secret: ghs_0123456789abcdefABCDEF0123".into() },
	"trigger instruction"
)]
fn a_template_with_a_credential_is_refused(#[case] change: fn(&mut Template), #[case] part: &str) {
	// Arrange
	let mut template = template();
	change(&mut template);
	// Act
	let error = template.validate(now()).unwrap_err().to_string();
	// Assert
	assert!(error.contains("carries no secrets"), "{error}");
	assert!(error.contains(part), "{error}");
}

#[rstest]
#[case::router_too_large(|t: &mut Template| t.router = Some(script(&"#".repeat(MAX_SOURCE_BYTES + 1))))]
#[case::router_with_nul(|t: &mut Template| t.router = Some(script("print(1)\0")))]
#[case::watcher_too_often(|t: &mut Template| t.watcher.as_mut().unwrap().every_seconds = 59)]
#[case::empty_mission(|t: &mut Template| t.mission = " ".into())]
#[case::long_title(|t: &mut Template| t.title = "x".repeat(crate::duties::TITLE_MAX + 1))]
#[case::bad_cron(|t: &mut Template| t.triggers[0] = ExpectedTrigger::Cron {
	expression: "61 * * * *".into(),
	time_zone: "UTC".into(),
	instruction: "x".into(),
})]
#[case::two_time_triggers(|t: &mut Template| t.triggers.push(ExpectedTrigger::Interval {
	every_seconds: 60,
	instruction: "x".into(),
}))]
#[case::long_instruction(|t: &mut Template| t.triggers[1] = ExpectedTrigger::Webhook {
	instruction: "x".repeat(INSTRUCTION_MAX + 1),
})]
#[case::agent_without_version(|t: &mut Template| t.agent = Some(SuggestedAgent {
	id: "ci-fixer".into(),
	version: String::new(),
}))]
fn a_template_keeps_the_limits_of_a_duty(#[case] change: fn(&mut Template)) {
	let mut template = template();
	change(&mut template);
	assert!(template.validate(now()).is_err(), "{template:?}");
}

#[rstest]
fn an_installed_template_is_a_duty_with_its_time_trigger_and_the_chosen_agent() {
	// Arrange
	let template = template();
	let agent = EntityRef {
		id: "local-fixer".into(),
		version: "3.0.0".into(),
	};
	// Act
	let duty = template.definition(agent.clone());
	// Assert: the work and the time trigger as given, the defaults of a new Duty.
	assert!(duty.validate(now()).is_ok());
	assert_eq!(duty.title, "GitHub CI duty");
	assert_eq!(duty.agent, agent);
	assert_eq!(duty.trigger, definition().trigger);
	assert_eq!(duty.router, template.router);
	assert_eq!(duty.watcher, template.watcher);
	assert_eq!(duty.approval_mode, None);
	assert_eq!(duty.max_lateness_seconds, DEFAULT_MAX_LATENESS_SECONDS);
	assert_eq!(duty.pause_after_failures, DEFAULT_PAUSE_AFTER_FAILURES);
	// Assert: the webhook is left for the person to create.
	assert_eq!(
		template.to_create(),
		vec![ExpectedTrigger::Webhook {
			instruction: "Add the URL as a workflow_run webhook in GitHub.".into()
		}]
	);
}

#[rstest]
fn a_package_carries_the_template_and_nothing_else() {
	// Arrange
	let template = template();
	// Act
	let package = template
		.package(Listing {
			package_id: "github-ci-duty",
			version: "1.0.0",
			author: "Acme",
			description: "",
		})
		.unwrap();
	// Assert
	assert_eq!(package.entity.kind, KIND);
	assert_eq!(package.entity.name["en"], "GitHub CI duty");
	assert_eq!(
		package.entity.description["en"],
		"Fix failing CI runs on main."
	);
	assert!(package.permissions.is_empty() && package.secrets.is_empty());
	assert_eq!(Template::from_package(&package).unwrap(), template);
}

#[rstest]
#[case::not_semver("1.0", "github-ci-duty", "Acme", "")]
#[case::bad_package_id("1.0.0", "github ci", "Acme", "")]
#[case::no_author("1.0.0", "github-ci-duty", " ", "")]
#[case::secret_description(
	"1.0.0",
	"github-ci-duty",
	"Acme",
	"uses ghp_0123456789abcdefABCDEF0123456789abcd"
)]
fn a_package_listing_is_checked(
	#[case] version: &str,
	#[case] package_id: &str,
	#[case] author: &str,
	#[case] description: &str,
) {
	assert!(
		template()
			.package(Listing {
				package_id,
				version,
				author,
				description,
			})
			.is_err()
	);
}

#[rstest]
#[case::secret_field(json!({"title":"t","mission":"m","webhook_secret":"abc"}))]
#[case::requester_field(json!({"title":"t","mission":"m","requester":"alice"}))]
#[case::trigger_secret(json!({"title":"t","mission":"m","triggers":[{"kind":"webhook","instruction":"x","secret":"abc"}]}))]
#[case::trigger_source_duty(json!({"title":"t","mission":"m","triggers":[{"kind":"duty_completed","instruction":"x","source_duty_id":"00000000-0000-0000-0000-000000000007"}]}))]
#[case::agent_credential(json!({"title":"t","mission":"m","agent":{"id":"a","version":"1.0.0","credential_id":"x"}}))]
fn a_package_whose_template_names_a_secret_like_field_is_refused(#[case] config: Value) {
	// Arrange
	let mut package = template()
		.package(Listing {
			package_id: "github-ci-duty",
			version: "1.0.0",
			author: "Acme",
			description: "",
		})
		.unwrap();
	package.entity.config = config;
	// Act
	let error = Template::from_package(&package).unwrap_err().to_string();
	// Assert
	assert!(error.contains("not a valid Duty template"), "{error}");
}

#[rstest]
fn a_package_that_names_tenant_secrets_is_refused() {
	let mut package = template()
		.package(Listing {
			package_id: "github-ci-duty",
			version: "1.0.0",
			author: "Acme",
			description: "",
		})
		.unwrap();
	package.secrets = vec!["GITHUB_TOKEN".into()];
	assert!(Template::from_package(&package).is_err());
}

#[rstest]
fn an_update_changes_the_work_and_keeps_the_requesters_choices() {
	// Arrange
	let mut next = template();
	next.mission = "Fix CI and report it.".into();
	next.watcher = None;
	let installed = template().definition(EntityRef {
		id: "local-fixer".into(),
		version: "3.0.0".into(),
	});
	// Act
	let (updated, retimed) = next.update_patch().apply(&installed);
	// Assert
	assert!(!retimed);
	assert_eq!(updated.mission, "Fix CI and report it.");
	assert_eq!(updated.watcher, None);
	assert_eq!(updated.router, next.router);
	assert_eq!(updated.title, installed.title);
	assert_eq!(updated.agent, installed.agent);
	assert_eq!(updated.trigger, installed.trigger);
}

fn version(package_id: &str, version: &str) -> crate::marketplace::Version {
	crate::marketplace::Version {
		key: format!("{package_id}@{version}"),
		repository: "aidash://deployment/node".into(),
		owner_tenant: "acme".into(),
		package_id: package_id.into(),
		version: version.into(),
		kind: KIND.into(),
		publisher: "alice".into(),
		source: EntityRef {
			id: package_id.into(),
			version: version.into(),
		},
		manifest_source: String::new(),
		digest: String::new(),
		dependencies: vec![],
		lineage: Default::default(),
	}
}

#[rstest]
#[case(version("github-ci-duty", "1.1.0"), true)]
#[case(version("github-ci-duty", "1.0.0"), false)]
#[case(version("github-ci-duty", "0.9.0"), false)]
#[case(version("gmail-triage", "2.0.0"), false)]
fn a_later_version_of_the_same_template_supersedes_it(
	#[case] candidate: crate::marketplace::Version,
	#[case] newer: bool,
) {
	let source = Source::of(&version("github-ci-duty", "1.0.0"), Some(Uuid::nil()));
	assert_eq!(source.superseded_by(&candidate), newer);
	assert_eq!(source.shown().install_key, None);
}
