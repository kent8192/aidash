use super::*;
fn form(value: Value) -> Form {
	serde_json::from_value(value).unwrap()
}
fn text(constraint: Value) -> Form {
	form(
		json!({"kind":"text","fields":[{"name":"answer","label":"Answer","constraint":constraint}]}),
	)
}
fn accept(value: Value) -> Answer {
	Answer {
		action: Action::Accept,
		value: Some(value),
		language: Language::Ja,
	}
}

#[test]
fn secret_destination_and_value_boundaries() {
	for host in [
		"::1",
		"[::1]",
		"127.0.0.1",
		"10.0.0.1",
		"2130706433",
		"localhost",
		"x.local",
		"x.internal",
		"x.home.arpa",
		"*.example.com",
		"auth.example.com/path",
	] {
		assert!(
			form(json!({"kind":"secret","label":"Key","name":"api","hosts":[host]}))
				.validate()
				.is_err(),
			"{host}"
		);
	}
	let f = form(json!({"kind":"secret","label":"Key","name":"api","hosts":["api.example.com"]}));
	for secret in [
		"a\nb",
		"a\rb",
		"a\0b",
		"a\u{85}b",
		"a\u{2028}b",
		"",
		&"é".repeat(4097),
	] {
		assert!(f.answer(&accept(json!(secret))).is_err());
	}
	assert_eq!(
		f.answer(&accept(json!("é".repeat(4096)))).unwrap(),
		json!({"name":"api","outcome":"saved"})
	);
	assert_eq!(
		f.answer(&Answer {
			action: Action::Decline,
			value: None,
			language: Language::En
		})
		.unwrap(),
		json!({"name":"api","outcome":"declined"})
	);
}
#[test]
fn checks_all_formats_and_gives_localized_errors() {
	for (format, good, bad) in [
		("email", "a@b.co", "a@@b.co"),
		("https_url", "https://example.org/p", "http://example.org"),
		("date", "2024-02-29", "2025-02-29"),
		("time", "23:59", "24:01"),
		("time_zone", "Asia/Tokyo", "Mars/Phobos"),
	] {
		let f = text(json!({"type":"format","format":format}));
		assert!(f.answer(&accept(json!({"answer":good}))).is_ok());
		let mut ans = accept(json!({"answer":bad}));
		let ja = f.answer(&ans).unwrap_err().to_string();
		ans.language = Language::En;
		assert_ne!(ja, f.answer(&ans).unwrap_err().to_string());
	}
}
#[test]
fn enforces_constraints_before_returning_person_data() {
	let f = text(json!({"type":"regex","pattern":"^\\p{Greek}+$","message":"Use Greek letters"}));
	assert_eq!(
		f.answer(&accept(json!({"answer":"α\tβ"}))).unwrap(),
		json!({"outcome":"answered","person_data":{"answer":"αβ"}})
	);
	assert_eq!(
		f.answer(&accept(json!({"answer":"abc"})))
			.unwrap_err()
			.to_string(),
		"Use Greek letters"
	);
	let f = text(json!({"type":"number","min":1,"max":5,"integer":true,"unit":"kg"}));
	for v in ["1", "5"] {
		assert!(f.answer(&accept(json!({"answer":v}))).is_ok());
	}
	for v in ["1.5", "6", "0", "nan"] {
		assert!(
			f.answer(&accept(json!({"answer":v})))
				.unwrap_err()
				.to_string()
				.contains("1〜5 kg")
		);
	}
	let f = text(Value::Null);
	assert!(
		f.answer(&accept(json!({"answer":"猫".repeat(2000)})))
			.is_ok()
	);
	assert!(
		f.answer(&accept(json!({"answer":"猫".repeat(2001)})))
			.is_err()
	);
	assert!(
		f.answer(&accept(json!({"answer":"ok","injected":"extra"})))
			.is_err()
	);
}
#[test]
fn choices_must_be_offered_and_have_the_right_cardinality() {
	for multiple in [true, false] {
		let f =
			form(json!({"kind":"choice","label":"Plan","multiple":multiple,"options":["a","b"]}));
		assert!(
			f.answer(&accept(if multiple {
				json!(["b", "a"])
			} else {
				json!("a")
			}))
			.is_ok()
		);
		for bad in [json!("c"), json!(["c"]), json!([]), json!(["a", "a"])] {
			assert!(f.answer(&accept(bad)).is_err());
		}
	}
}
#[test]
fn rejects_invalid_forms_before_asking() {
	for (pattern, message) in [
		("(".into(), "Explain".into()),
		("x".repeat(501), "Explain".into()),
		("x".into(), " ".into()),
		("x".into(), "x".repeat(201)),
	] {
		assert!(
			text(json!({"type":"regex","pattern":pattern,"message":message}))
				.validate()
				.is_err()
		);
	}
	let fields: Vec<_> = (0..11)
		.map(|i| json!({"name":format!("f{i}"),"label":"Field"}))
		.collect();
	assert!(
		form(json!({"kind":"text","fields":fields}))
			.validate()
			.is_err()
	);
	assert!(
		form(json!({"kind":"choice","label":"x".repeat(201),"options":["a"]}))
			.validate()
			.is_err()
	);
	assert!(
		form(
			json!({"kind":"choice","label":"x","options":(0..51).map(|n|n.to_string()).collect::<Vec<_>>()})
		)
		.validate()
		.is_err()
	);
}
