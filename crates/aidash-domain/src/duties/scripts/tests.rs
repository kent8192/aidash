use super::*;
use rstest::rstest;
use serde_json::json;

fn script(language: Language, source: &str) -> Script {
	Script {
		language,
		source: source.into(),
	}
}

#[rstest]
#[case(script(Language::Python, "print('{}')"), true)]
#[case(script(Language::Shell, "echo '{\"action\":\"drop\"}'"), true)]
#[case(script(Language::Shell, "   \n"), false)]
#[case(script(Language::Python, "print(1)\0"), false)]
#[case(script(Language::Python, &"#".repeat(MAX_SOURCE_BYTES)), true)]
#[case(script(Language::Python, &"#".repeat(MAX_SOURCE_BYTES + 1)), false)]
fn a_script_has_source_within_64_kib_and_no_nul(#[case] script: Script, #[case] valid: bool) {
	assert_eq!(
		script.validate().is_ok(),
		valid,
		"{:?}",
		script.source.len()
	);
}

#[rstest]
#[case(59, false)]
#[case(60, true)]
#[case(super::super::MAX_INTERVAL_SECONDS, true)]
#[case(super::super::MAX_INTERVAL_SECONDS + 1, false)]
fn a_watcher_runs_every_minute_to_one_year(#[case] every_seconds: i64, #[case] valid: bool) {
	let watcher = Watcher {
		script: script(Language::Shell, "echo '[]'"),
		every_seconds,
	};
	assert_eq!(watcher.validate().is_ok(), valid);
}

#[rstest]
fn a_script_choice_sets_or_removes() {
	let set: ScriptChoice =
		serde_json::from_value(json!({"language":"python","source":"print(1)"})).unwrap();
	assert_eq!(set, ScriptChoice::Set(script(Language::Python, "print(1)")));
	let removed: ScriptChoice = serde_json::from_value(json!({"kind":"none"})).unwrap();
	assert_eq!(removed, ScriptChoice::Remove(NoTrigger::None));
	assert!(
		serde_json::from_value::<ScriptChoice>(json!({"language":"ruby","source":"p 1"})).is_err()
	);
}

#[rstest]
#[case(r#"{"action":"drop"}"#, RouterAction::Drop {})]
#[case("  {\"action\":\"act\"}\n", RouterAction::Act {})]
#[case(
	r#"{"action":"task","title":"Fix CI","description":"Run 42 failed"}"#,
	RouterAction::Task { title: "Fix CI".into(), description: "Run 42 failed".into() }
)]
#[case(
	r#"{"action":"task","title":"Fix CI"}"#,
	RouterAction::Task { title: "Fix CI".into(), description: String::new() }
)]
#[case(
	r#"{"action":"deliver","task_id":"00000000-0000-0000-0000-000000000001","input":"CI passed"}"#,
	RouterAction::Deliver { task_id: Uuid::from_u128(1), input: "CI passed".into() }
)]
fn a_router_prints_one_action(#[case] stdout: &str, #[case] expected: RouterAction) {
	assert_eq!(parse_router(stdout).unwrap(), expected);
}

#[rstest]
#[case("")]
#[case("drop")]
#[case(r#"{"action":"merge"}"#)]
#[case(r#"{"action":"drop","extra":1}"#)]
#[case(r#"{"action":"task"}"#)]
#[case(r#"{"action":"task","title":"  "}"#)]
#[case(r#"{"action":"deliver","task_id":"not-a-uuid","input":"x"}"#)]
#[case(r#"{"action":"deliver","task_id":"00000000-0000-0000-0000-000000000001","input":""}"#)]
#[case(r#"{"action":"drop"} {"action":"act"}"#)]
#[case(r#"[{"action":"drop"}]"#)]
fn anything_else_a_router_prints_is_refused_with_a_reason(#[case] stdout: &str) {
	let reason = parse_router(stdout).unwrap_err();
	assert!(!reason.is_empty());
	assert!(reason.chars().count() < 400, "{reason}");
}

#[rstest]
fn a_router_reads_the_event_its_trigger_and_the_open_tasks() {
	let stdin = router_stdin(
		&json!({"conclusion":"failure"}),
		(Uuid::from_u128(7), "webhook"),
		&[OpenTask {
			task_id: Uuid::from_u128(9),
			title: "Fix CI".into(),
		}],
	)
	.unwrap();
	let value: Value = serde_json::from_str(&stdin).unwrap();
	assert_eq!(value["event"]["conclusion"], "failure");
	assert_eq!(value["trigger"]["kind"], "webhook");
	assert_eq!(value["trigger"]["id"], Uuid::from_u128(7).to_string());
	assert_eq!(value["open_tasks"][0]["title"], "Fix CI");
}

#[rstest]
fn a_watcher_reports_an_array_of_event_objects() {
	let output = parse_watcher(r#"[{"id":1},{"id":2}]"#).unwrap();
	assert_eq!(output.events, vec![json!({"id":1}), json!({"id":2})]);
	assert_eq!(output.dropped, None);
	assert_eq!(parse_watcher(" \n").unwrap().events, Vec::<Value>::new());
	assert!(parse_watcher(r#"{"id":1}"#).is_err());
}

#[rstest]
fn a_watcher_keeps_32_events_of_at_most_256_kib() {
	let mut entries: Vec<Value> = (0..40).map(|id| json!({"id": id})).collect();
	entries.insert(
		3,
		json!({"blob": "x".repeat(super::super::events::MAX_EVENT_BYTES)}),
	);
	entries.insert(5, json!("not an object"));
	let output = parse_watcher(&serde_json::to_string(&entries).unwrap()).unwrap();
	assert_eq!(output.events.len(), MAX_WATCHER_EVENTS);
	assert_eq!(output.events[0], json!({"id": 0}));
	let dropped = output.dropped.unwrap();
	assert!(dropped.contains("8 past the 32-event limit"), "{dropped}");
	assert!(dropped.contains("1 over"), "{dropped}");
	assert!(dropped.contains("1 not JSON objects"), "{dropped}");
}

fn decode_writes(commands: &[String], name: &str) -> Vec<u8> {
	// Every chunk is a quoted base64 literal after the file's write command.
	let mut encoded = String::new();
	for command in commands {
		let lines: Vec<&str> = command.lines().collect();
		for (index, line) in lines.iter().enumerate() {
			let shell = line.starts_with("printf '%s' '") && line.ends_with(&format!("/{name}'"));
			let python =
				line.starts_with("with open(\"") && line.contains(&format!("/{name}\", \""));
			if shell {
				encoded.push_str(line.split('\'').nth(3).unwrap());
			} else if python {
				// The data is on the next line of the same command.
				encoded.push_str(lines[index + 1].split('"').nth(1).unwrap());
			}
		}
	}
	STANDARD.decode(encoded).unwrap()
}

#[rstest]
#[case(Language::Shell)]
#[case(Language::Python)]
fn a_small_script_runs_in_one_call(#[case] language: Language) {
	let invocation = Invocation {
		language,
		source: "print('hi')".into(),
		stdin: r#"{"event":{}}"#.into(),
	};
	let commands = plan(&invocation, &directory(Uuid::nil()));
	assert_eq!(commands.len(), 1);
	assert!(commands[0].len() <= 64 * 1024);
	assert_eq!(decode_writes(&commands, "script"), b"print('hi')");
	assert_eq!(decode_writes(&commands, "stdin"), br#"{"event":{}}"#);
}

#[rstest]
#[case(Language::Shell)]
#[case(Language::Python)]
fn a_64_kib_script_and_a_256_kib_event_fit_the_command_limit(#[case] language: Language) {
	let source = "#".repeat(MAX_SOURCE_BYTES);
	let stdin = "e".repeat(256 * 1024 + 1000);
	let invocation = Invocation {
		language,
		source: source.clone(),
		stdin: stdin.clone(),
	};
	let commands = plan(&invocation, &directory(Uuid::nil()));
	assert!(commands.len() > 2);
	assert!(commands.iter().all(|command| command.len() <= 64 * 1024));
	assert_eq!(decode_writes(&commands, "script"), source.as_bytes());
	assert_eq!(decode_writes(&commands, "stdin"), stdin.as_bytes());
	// The script runs last.
	assert!(
		commands
			.last()
			.unwrap()
			.contains("aidash-duty-script-output")
	);
}

#[rstest]
fn the_calls_of_a_run_have_stable_distinct_keys() {
	let run = Uuid::from_u128(5);
	assert_eq!(call_key(run, 0), call_key(run, 0));
	assert_ne!(call_key(run, 0), call_key(run, 1));
	assert_ne!(call_key(run, 0), call_key(Uuid::from_u128(6), 0));
}

#[rstest]
#[case("\u{1e}aidash-duty-script-output\n{\"action\":\"drop\"}\n", Ok("{\"action\":\"drop\"}\n".to_owned()))]
#[case(
	"[The Python session restarted]\n\u{1e}aidash-duty-script-output\n[]",
	Ok("[]".to_owned())
)]
#[case(
	"\u{1e}aidash-duty-script-output\npartial\n\u{1e}aidash-duty-script-error 2\nTraceback: boom\n",
	Err("the script exited with code 2: Traceback: boom".to_owned())
)]
#[case(
	"\u{1e}aidash-duty-script-output\n\n\u{1e}aidash-duty-script-error 1\n",
	Err("the script exited with code 1".to_owned())
)]
#[case("base64: invalid input", Err("the script did not run: base64: invalid input".to_owned()))]
fn only_the_scripts_own_stdout_is_its_output(
	#[case] output: &str,
	#[case] expected: std::result::Result<String, String>,
) {
	assert_eq!(script_output(output), expected);
}

/// Each language's operation reads the code from its own field: the Python
/// operation refuses `command`, the shell operation `code` (#1011).
#[rstest]
#[case(Language::Python, "code_interpreter", "code")]
#[case(Language::Shell, "shell", "command")]
fn each_language_names_its_operation_and_code_field(
	#[case] language: Language,
	#[case] operation: &str,
	#[case] field: &str,
) {
	assert_eq!(language.operation(), operation);
	assert_eq!(language.code_field(), field);
}
