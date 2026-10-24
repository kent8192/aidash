use super::*;
use crate::ports::authorization::visibility::provenance::{LocalOutputScope, OutputScope};
use async_trait::async_trait;
use rstest::rstest;

#[derive(Default)]
struct Scope {
	grants: Vec<Uuid>,
	producers: Vec<Uuid>,
	denied: Option<Uuid>,
	fail: Option<&'static str>,
	calls: Vec<String>,
	scopes: Vec<(Uuid, String, Uuid)>,
}
impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"output adapter fault",
			))));
		}
		Ok(())
	}
}
#[async_trait]
impl LocalOutputScope for Scope {
	async fn producers(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Vec<Uuid>> {
		self.touch("producers")?;
		self.scopes.push((workspace, kind.into(), id));
		Ok(self.producers.clone())
	}
	async fn producer_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		self.touch("producer")?;
		self.calls.push(format!("producer:{run}"));
		Ok(self.denied != Some(run))
	}
}
#[async_trait]
impl OutputScope for Scope {
	async fn remote_grants(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Vec<Uuid>> {
		self.touch("grants")?;
		self.scopes.push((workspace, kind.into(), id));
		Ok(self.grants.clone())
	}
	async fn grant_visible(&mut self, grant: Uuid) -> Result<bool> {
		self.touch("grant")?;
		self.calls.push(format!("grant:{grant}"));
		Ok(self.denied != Some(grant))
	}
}
fn fixture() -> Scope {
	Scope {
		grants: vec![Uuid::from_u128(1), Uuid::from_u128(2)],
		producers: vec![Uuid::from_u128(3), Uuid::from_u128(4)],
		..Scope::default()
	}
}

#[rstest]
#[tokio::test]
async fn all_remote_grants_are_authorized_before_loading_or_reading_local_producers() {
	let mut scope = fixture();
	let workspace = Uuid::from_u128(5);
	let id = Uuid::from_u128(6);
	assert!(
		output_visible(&mut scope, workspace, "artifact", id)
			.await
			.unwrap()
	);
	assert_eq!(
		scope.scopes,
		vec![
			(workspace, "artifact".into(), id),
			(workspace, "artifact".into(), id)
		]
	);
	assert_eq!(
		scope.calls,
		vec![
			"grants".into(),
			"grant".into(),
			format!("grant:{}", Uuid::from_u128(1)),
			"grant".into(),
			format!("grant:{}", Uuid::from_u128(2)),
			"producers".into(),
			"producer".into(),
			format!("producer:{}", Uuid::from_u128(3)),
			"producer".into(),
			format!("producer:{}", Uuid::from_u128(4))
		]
	);
}

#[rstest]
#[case(1)]
#[case(2)]
#[tokio::test]
async fn denied_remote_grants_stop_before_any_local_producer_lookup(#[case] denied: u128) {
	let mut scope = fixture();
	scope.denied = Some(Uuid::from_u128(denied));
	assert!(
		!output_visible(
			&mut scope,
			Uuid::from_u128(5),
			"message",
			Uuid::from_u128(6)
		)
		.await
		.unwrap()
	);
	assert!(!scope.calls.contains(&"producers".into()));
	assert_eq!(
		scope.calls.iter().filter(|c| c.as_str() == "grant").count(),
		denied as usize
	);
}

#[rstest]
#[case(3)]
#[case(4)]
#[tokio::test]
async fn a_denied_local_producer_hides_its_output_after_grant_checks(#[case] denied: u128) {
	let mut scope = fixture();
	scope.denied = Some(Uuid::from_u128(denied));
	assert!(
		!output_visible(&mut scope, Uuid::from_u128(5), "task", Uuid::from_u128(6))
			.await
			.unwrap()
	);
	assert_eq!(
		scope.calls.iter().filter(|c| c.as_str() == "grant").count(),
		2
	);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.as_str() == "producer")
			.count(),
		(denied - 2) as usize
	);
}

#[rstest]
#[tokio::test]
async fn the_native_local_reader_checks_only_its_local_producer_journal() {
	let mut scope = fixture();
	scope.denied = Some(Uuid::from_u128(1));
	assert!(
		local_output_visible(
			&mut scope,
			Uuid::from_u128(5),
			"artifact",
			Uuid::from_u128(6)
		)
		.await
		.unwrap()
	);
	assert!(!scope.calls.contains(&"grants".into()));
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.as_str() == "producer")
			.count(),
		2
	);
}

#[rstest]
#[tokio::test]
async fn outputs_without_provenance_members_are_readable_without_external_checks() {
	let mut scope = Scope::default();
	assert!(
		output_visible(&mut scope, Uuid::from_u128(5), "task", Uuid::from_u128(6))
			.await
			.unwrap()
	);
	assert_eq!(scope.calls, vec!["grants", "producers"]);
}

#[rstest]
#[case("grants")]
#[case("grant")]
#[case("producers")]
#[case("producer")]
#[tokio::test]
async fn output_membership_and_read_failures_keep_their_error_identity(
	#[case] boundary: &'static str,
) {
	let mut scope = fixture();
	scope.fail = Some(boundary);
	let error = output_visible(&mut scope, Uuid::from_u128(5), "task", Uuid::from_u128(6))
		.await
		.unwrap_err();
	assert!(matches!(error,Error::Port(ref error) if error.to_string()=="output adapter fault"));
	assert_eq!(scope.calls.last().map(String::as_str), Some(boundary));
}
