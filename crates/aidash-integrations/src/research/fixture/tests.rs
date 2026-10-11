use super::*;
use rstest::rstest;
use serde_json::json;

#[rstest]
#[tokio::test]
async fn search_ranks_folded_term_matches_and_sends_nothing_out() {
	let fixture = FixtureResearch::default();
	let query = SearchQuery::parse(&json!({"query":"STRASSE line"}), None).unwrap();
	assert_eq!(fixture.disclosure(&query).unwrap().destination, DESTINATION);
	assert_eq!(fixture.account(), Ok(None));
	let results = fixture.search(&query).await.unwrap();
	assert_eq!(results.candidates.len(), 1);
	assert_eq!(
		results.candidates[0].url,
		"https://research-fixture.example/strasse"
	);
	let none = SearchQuery::parse(&json!({"query":"absent"}), None).unwrap();
	assert!(fixture.search(&none).await.unwrap().candidates.is_empty());
}

#[rstest]
#[tokio::test]
async fn pages_are_served_only_from_the_corpus() {
	let fixture = FixtureResearch::default();
	let url = Url::parse("https://research-fixture.example/strasse").unwrap();
	let page = fixture.fetch(&url).await.unwrap();
	assert!(page.validate().is_ok());
	assert_eq!(page.lines[1].page, Some(2));
	assert_eq!(fixture.fetch(&url).await.unwrap(), page, "deterministic");
	assert_eq!(
		fixture
			.fetch(&Url::parse("https://research-fixture.example/missing").unwrap())
			.await,
		Err(Failure::ProviderRequestRejected)
	);
	assert_eq!(
		fixture.admit(&Url::parse("http://localhost/").unwrap()),
		Err(Failure::NonPublicDestination)
	);
}
