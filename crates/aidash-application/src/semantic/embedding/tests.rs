use super::*;
use aidash_domain::semantic::Embedding;
use rstest::rstest;
use std::sync::{Arc, Mutex};

type Calls = Arc<Mutex<Vec<&'static str>>>;
fn configuration() -> EmbeddingConfig {
	EmbeddingConfig {
		provider: "openai".into(),
		endpoint: "https://embedding.invalid".into(),
		credential_env: None,
		model: "fixture".into(),
		model_version: "1".into(),
		dimensions: 2,
	}
}
#[derive(Debug, thiserror::Error)]
#[error("retained adapter failure")]
struct AdapterFailure;
struct Allowance {
	calls: Calls,
	reported: Arc<Mutex<Vec<Option<u64>>>>,
	settlement_fails: bool,
}
#[async_trait]
impl EmbeddingAllowance for Allowance {
	async fn settle(self: Box<Self>, tokens: Option<u64>) -> Result<()> {
		self.calls.lock().unwrap().push("settle");
		self.reported.lock().unwrap().push(tokens);
		if self.settlement_fails {
			Err(Error::Port(Box::new(AdapterFailure)))
		} else {
			Ok(())
		}
	}
}
struct Scope {
	calls: Calls,
	reservation: bool,
	reservation_fails: bool,
	settlement_fails: bool,
	reported: Arc<Mutex<Vec<Option<u64>>>>,
}
#[async_trait]
impl SemanticEmbeddingScope for Scope {
	async fn reserve(
		&mut self,
		workspace: Uuid,
		config: &EmbeddingConfig,
		text: &str,
		origin: Origin,
	) -> Result<Option<Box<dyn EmbeddingAllowance>>> {
		self.calls.lock().unwrap().push("reserve");
		assert_eq!(workspace, Uuid::from_u128(1));
		assert_eq!(*config, configuration());
		assert_eq!(text, "query");
		assert!(matches!(origin, Origin::Query(Some(run)) if run==Uuid::from_u128(2)));
		if self.reservation_fails {
			return Err(Error::Forbidden);
		}
		Ok(self.reservation.then(|| {
			Box::new(Allowance {
				calls: self.calls.clone(),
				reported: self.reported.clone(),
				settlement_fails: self.settlement_fails,
			}) as Box<dyn EmbeddingAllowance>
		}))
	}
}
struct Provider {
	calls: Calls,
	reported: Option<u64>,
	fails: bool,
}
#[async_trait]
impl EmbeddingProvider for Provider {
	async fn embed(&self, config: &EmbeddingConfig, text: &str) -> Result<Embedding> {
		self.calls.lock().unwrap().push("provider");
		assert_eq!(*config, configuration());
		assert_eq!(text, "query");
		if self.fails {
			Err(Error::External("sensitive provider diagnostic".into()))
		} else {
			Ok(Embedding {
				vector: vec![0.25, 0.75],
				tokens: self.reported,
			})
		}
	}
}
fn fixture() -> (Calls, Scope, Provider) {
	let calls = Calls::default();
	(
		calls.clone(),
		Scope {
			calls: calls.clone(),
			reservation: true,
			reservation_fails: false,
			settlement_fails: false,
			reported: Arc::new(Mutex::new(vec![])),
		},
		Provider {
			calls,
			reported: Some(1),
			fails: false,
		},
	)
}
#[rstest]
#[case::legacy(false, Some(1))]
#[case::reported(true, Some(1))]
#[case::zero_usage(true, Some(0))]
#[case::missing_usage(true, None)]
#[tokio::test]
async fn embedding_settles_complete_output_only_after_current_authority_reservation(
	#[case] reservation: bool,
	#[case] tokens: Option<u64>,
) {
	// Arrange
	let (calls, mut scope, mut provider) = fixture();
	scope.reservation = reservation;
	provider.reported = tokens;
	// Act
	let vector = invoke(
		&mut scope,
		&provider,
		Uuid::from_u128(1),
		&configuration(),
		"query",
		Origin::Query(Some(Uuid::from_u128(2))),
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(vector, [0.25, 0.75]);
	assert_eq!(
		*calls.lock().unwrap(),
		if reservation {
			vec!["reserve", "provider", "settle"]
		} else {
			vec!["reserve", "provider"]
		}
	);
	assert_eq!(
		*scope.reported.lock().unwrap(),
		if reservation { vec![tokens] } else { vec![] }
	);
}
#[rstest]
#[tokio::test]
async fn refused_allowance_stops_before_provider_effects() {
	let (calls, mut scope, provider) = fixture();
	scope.reservation_fails = true;
	assert!(matches!(
		invoke(
			&mut scope,
			&provider,
			Uuid::from_u128(1),
			&configuration(),
			"query",
			Origin::Query(Some(Uuid::from_u128(2)))
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(*calls.lock().unwrap(), ["reserve"]);
	assert!(scope.reported.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn provider_failure_keeps_conservative_charge_and_hides_provider_details() {
	let (calls, mut scope, mut provider) = fixture();
	provider.fails = true;
	let error = invoke(
		&mut scope,
		&provider,
		Uuid::from_u128(1),
		&configuration(),
		"query",
		Origin::Query(Some(Uuid::from_u128(2))),
	)
	.await
	.unwrap_err();
	assert!(matches!(error, Error::SemanticUnavailable));
	assert_eq!(
		error.to_string(),
		"semantic backend unavailable or invalid; inspect index status and retry"
	);
	assert_eq!(*calls.lock().unwrap(), ["reserve", "provider"]);
	assert!(scope.reported.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn settlement_error_keeps_adapter_identity_and_withholds_embedding() {
	let (calls, mut scope, provider) = fixture();
	scope.settlement_fails = true;
	let error = invoke(
		&mut scope,
		&provider,
		Uuid::from_u128(1),
		&configuration(),
		"query",
		Origin::Query(Some(Uuid::from_u128(2))),
	)
	.await
	.unwrap_err();
	assert!(matches!(error, Error::Port(error) if error.is::<AdapterFailure>()));
	assert_eq!(*calls.lock().unwrap(), ["reserve", "provider", "settle"]);
	assert_eq!(*scope.reported.lock().unwrap(), [Some(1)]);
}
