//! Persistence owns the distinction between an old admission and manual readmission.
use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use crate::apps::identity::models::DashboardIdentity;
use aidash_application::ports::{
	accounts::Authority,
	authorization::dashboard::{AccountPolicy, Issuer},
};
use aidash_domain::accounts::DurableOrigin;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use rstest::rstest;
use std::sync::{
	Arc, Mutex,
	atomic::{AtomicBool, Ordering},
};

struct Online {
	current: Mutex<DurableOrigin>,
	unavailable: AtomicBool,
	session: Uuid,
}
#[async_trait]
impl Authority for Online {
	async fn validate_interactive(
		&self,
		identity: Uuid,
		session: Uuid,
	) -> aidash_application::Result<()> {
		if identity != self.current.lock().unwrap().identity_id || session != self.session {
			return Err(aidash_application::Error::Forbidden);
		}
		self.validate_origin(identity).await
	}
	async fn validate_origin(&self, identity: Uuid) -> aidash_application::Result<()> {
		if self.unavailable.load(Ordering::SeqCst) {
			return Err(aidash_application::Error::IdentityStatusUnavailable);
		}
		if identity != self.current.lock().unwrap().identity_id {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(())
	}
	async fn origin_binding(&self, identity: Uuid) -> aidash_application::Result<DurableOrigin> {
		self.validate_origin(identity).await?;
		Ok(self.current.lock().unwrap().clone())
	}
	async fn validate_bound_origin(
		&self,
		origin: &DurableOrigin,
	) -> aidash_application::Result<()> {
		self.validate_origin(origin.identity_id).await?;
		if *origin != *self.current.lock().unwrap() {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(())
	}
}

#[rstest]
#[tokio::test]
async fn restored_authority_does_not_rebind_old_work_and_manual_resume_retains_its_source(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let scope = database.tenant("acme").await;
	let pool = database.pool().await;
	let policy = serde_json::from_value(
		serde_json::json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[]}),
	)
	.unwrap();
	crate::authorization::Authorization { pool: pool.clone() }
		.replace("acme", 0, policy, "deployment_administrator")
		.await
		.unwrap();
	let credential = crate::authorization::Authorization { pool: pool.clone() }
		.issue_credential("acme", "alice", 3600, "deployment_administrator")
		.await
		.unwrap()
		.credential
		.id;
	let source = Uuid::new_v4();
	let mapping = Uuid::new_v4();
	let run = Uuid::new_v4();
	let account = Uuid::new_v4();
	let now = Utc::now();
	database
		.fixture(async |tx| {
			let identity = DashboardIdentity::build()
				.id(source)
				.issuer("upstream")
				.subject("original")
				.valid_since(None)
				.verified_email(None)
				.display_name(None)
				.last_valid_at(Some(now))
				.disabled_at(None)
				.display_observed_at(None)
				.display_erased_at(None)
				.display_erased_by(None)
				.finish();
			DashboardIdentity::objects()
				.create_with_conn(tx, &identity)
				.await?;
			let mapping = DashboardMapping::build()
				.id(mapping)
				.identity(source)
				.tenant("acme")
				.subject("alice")
				.credential(credential)
				.enabled(true)
				.revision(1)
				.finish();
			DashboardMapping::objects()
				.create_with_conn(tx, &mapping)
				.await?;
			Ok(())
		})
		.await;
	let mut tx = database.begin_in(&scope).await;
	let insert = Query::insert()
		.into_table(Alias::new("runs"))
		.columns(
			[
				"id",
				"tenant",
				"task_id",
				"workspace_id",
				"node",
				"home_node",
				"agent_id",
				"agent_version",
				"control",
				"error",
			]
			.map(Alias::new),
		)
		.values_panic([
			reinhardt::query::IntoValue::into_value(run),
			"acme".into(),
			Uuid::new_v4().into(),
			Uuid::new_v4().into(),
			aidash_domain::nodes::NodeName::derived("acme")
				.to_string()
				.into(),
			"aidash://fixture/acme".into(),
			"agent".into(),
			"1.0.0".into(),
			"PAUSED".into(),
			"identity status unavailable".into(),
		])
		.to_string(PostgresQueryBuilder);
	crate::database::native::query(&insert)
		.execute(&mut *tx)
		.await
		.unwrap();
	let insert = Query::insert()
		.into_table(Alias::new("dashboard_execution_origins"))
		.columns(
			[
				"run_id",
				"tenant",
				"identity_id",
				"mapping_id",
				"account_id",
				"suspension_epoch",
				"source_disable_epoch",
			]
			.map(Alias::new),
		)
		.values_panic([
			reinhardt::query::IntoValue::into_value(run),
			"acme".into(),
			source.into(),
			mapping.into(),
			account.into(),
			0_i64.into(),
			0_i64.into(),
		])
		.to_string(PostgresQueryBuilder);
	crate::database::native::query(&insert)
		.execute(&mut *tx)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let online = Arc::new(Online {
		current: Mutex::new(DurableOrigin {
			identity_id: source,
			account_id: Some(account),
			suspension_epoch: Some(0),
			source_disable_epoch: Some(0),
		}),
		unavailable: false.into(),
		session: Uuid::new_v4(),
	});
	let policy = AccountPolicy {
		issuers: Arc::new(Issuer("upstream".into())),
		authority: Some(online.clone()),
		personal_tenants: false,
		demo_data: false,
	};
	let mut tx = database.begin_in(&scope).await;
	DashboardExecutionOrigin::require_current(&mut *tx, run, Some(&policy))
		.await
		.unwrap();
	online.unavailable.store(true, Ordering::SeqCst);
	assert!(matches!(
		DashboardExecutionOrigin::require_current(&mut *tx, run, Some(&policy)).await,
		Err(crate::Error::IdentityStatusUnavailable)
	));
	// A suspension and restore happened while this product could not observe it.
	online.current.lock().unwrap().suspension_epoch = Some(1);
	online.unavailable.store(false, Ordering::SeqCst);
	assert!(matches!(
		DashboardExecutionOrigin::require_current(&mut *tx, run, Some(&policy)).await,
		Err(crate::Error::Forbidden)
	));
	let mut identity = SubjectIdentity {
		http_session: None,
		credential_id: credential,
		tenant: "acme".into(),
		subject: "alice".into(),
	};
	assert!(matches!(
		DashboardExecutionOrigin::renew_manual(&mut *tx, run, &identity, Some(&policy)).await,
		Err(crate::Error::Forbidden)
	));
	identity.http_session = Some(crate::authorization::identity::HttpSession {
		id: Uuid::new_v4(),
		provider_session: Some(online.session),
		common_authority: Some(online.clone()),
		identity_id: Uuid::new_v4(),
		idle_seconds: 1800,
		access_expires_at: Some(now + Duration::minutes(5)),
	});
	assert!(matches!(
		DashboardExecutionOrigin::renew_manual(&mut *tx, run, &identity, Some(&policy)).await,
		Err(crate::Error::Forbidden)
	));
	identity.http_session.as_mut().unwrap().identity_id = source;
	DashboardExecutionOrigin::renew_manual(&mut *tx, run, &identity, Some(&policy))
		.await
		.unwrap();
	DashboardExecutionOrigin::require_current(&mut *tx, run, Some(&policy))
		.await
		.unwrap();
	online.current.lock().unwrap().source_disable_epoch = Some(1);
	assert!(matches!(
		DashboardExecutionOrigin::require_current(&mut *tx, run, Some(&policy)).await,
		Err(crate::Error::Forbidden)
	));
	tx.commit().await.unwrap();
	let saved = database
		.fixture(async |tx| {
			Ok(DashboardExecutionOrigin::objects()
				.filter(DashboardExecutionOrigin::field_run_id().eq(run))
				.get_with_db(tx)
				.await?)
		})
		.await;
	assert_eq!(saved.identity_id(), source);
	assert_eq!(saved.mapping_id(), mapping);
	assert_eq!(saved.account_id, Some(account));
	assert_eq!(saved.suspension_epoch, Some(1));
	assert_eq!(saved.source_disable_epoch, Some(0));

	// A local issuer retirement is independent of the shared stop epochs. A
	// restore must preserve it even if an active worker never observed it.
	let outage = Uuid::new_v4();
	let terminal = Uuid::new_v4();
	let cancelled = Uuid::new_v4();
	let worker = Uuid::new_v4();
	let mut tx = database.begin_in(&scope).await;
	let update = Query::update()
		.table(Alias::new("runs"))
		.value(Alias::new("control"), "ACTIVE")
		.value(Alias::new("error"), None::<String>)
		.value(Alias::new("lease_owner"), worker)
		.value(Alias::new("lease_until"), now + Duration::minutes(5))
		.and_where(Expr::col("id").eq(Expr::value(run)))
		.to_string(PostgresQueryBuilder);
	crate::database::native::query(&update)
		.execute(&mut *tx)
		.await
		.unwrap();
	for (id, phase, control, error) in [
		(
			outage,
			"READY",
			"PAUSED",
			Some("identity status unavailable"),
		),
		(terminal, "COMPLETED", "ACTIVE", None),
		(cancelled, "READY", "CANCELLED", Some("cancelled")),
	] {
		let insert = Query::insert()
			.into_table(Alias::new("runs"))
			.columns(
				[
					"id",
					"tenant",
					"task_id",
					"workspace_id",
					"node",
					"home_node",
					"agent_id",
					"agent_version",
					"phase",
					"control",
					"error",
				]
				.map(Alias::new),
			)
			.values_panic([
				reinhardt::query::IntoValue::into_value(id),
				"acme".into(),
				Uuid::new_v4().into(),
				Uuid::new_v4().into(),
				aidash_domain::nodes::NodeName::derived("acme")
					.to_string()
					.into(),
				"aidash://fixture/acme".into(),
				"agent".into(),
				"1.0.0".into(),
				phase.into(),
				control.into(),
				reinhardt::query::IntoValue::into_value(error),
			])
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&insert)
			.execute(&mut *tx)
			.await
			.unwrap();
		let columns = [
			"tenant",
			"identity_id",
			"mapping_id",
			"account_id",
			"suspension_epoch",
			"source_disable_epoch",
		];
		let insert = Query::insert()
			.into_table(Alias::new("dashboard_execution_origins"))
			.columns(std::iter::once("run_id").chain(columns).map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.columns(columns.map(Alias::new))
					.from(Alias::new("dashboard_execution_origins"))
					.and_where(Expr::col("run_id").eq(Expr::value(run)))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&insert)
			.execute(&mut *tx)
			.await
			.unwrap();
	}
	tx.commit().await.unwrap();
	database
		.fixture(async |tx| {
			DashboardIdentity::objects()
				.filter(DashboardIdentity::field_id().eq(source))
				.update_fields_with_conn(tx, [(DashboardIdentity::field_disabled_at(), Some(now))])
				.await?;
			Ok(())
		})
		.await;
	let all_tenants = aidash_application::tenancy::TenancyScope::Deployment(
		aidash_application::tenancy::DeploymentScopeReason::IdentityStatus,
	);
	pool.atomic(&all_tenants, async |tx| {
		DashboardIdentity::restore_common(tx, source, Utc::now()).await
	})
	.await
	.unwrap();
	for id in [run, outage] {
		assert!(!Run::resume_identity_pause(&pool, &scope, id).await.unwrap());
	}
	let rows = pool
		.atomic(&all_tenants, async |tx| {
			Ok(Run::objects()
				.filter(Run::field_id().is_in([run, outage, terminal, cancelled]))
				.all_with_db(tx)
				.await?)
		})
		.await
		.unwrap();
	for id in [run, outage] {
		let row = rows.iter().find(|row| row.id == id).unwrap();
		assert_eq!(row.control, RunControl::Paused);
		assert_eq!(row.error.as_deref(), Some("external identity disabled"));
		assert_eq!(row.revision, 1);
	}
	assert_eq!(
		rows.iter().find(|row| row.id == run).unwrap().lease_owner,
		Some(worker)
	);
	let completed = rows.iter().find(|row| row.id == terminal).unwrap();
	assert_eq!(
		completed.phase,
		crate::apps::execution::services::states::RunPhase::Completed
	);
	assert_eq!(completed.control, RunControl::Active);
	assert_eq!(completed.error, None);
	assert_eq!(completed.revision, 0);
	let cancelled = rows.iter().find(|row| row.id == cancelled).unwrap();
	assert_eq!(cancelled.control, RunControl::Cancelled);
	assert_eq!(cancelled.error.as_deref(), Some("cancelled"));
	assert_eq!(cancelled.revision, 0);
	let restored = database
		.fixture(async |tx| DashboardIdentity::find(tx, source).await)
		.await;
	assert_eq!(restored.disabled_at, None);
	assert_eq!(online.current.lock().unwrap().suspension_epoch, Some(1));
	assert_eq!(online.current.lock().unwrap().source_disable_epoch, Some(1));
}
