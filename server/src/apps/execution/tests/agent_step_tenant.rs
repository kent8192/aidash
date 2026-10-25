//! Which Tenant a step's inference names when it reads the Run's Provider
//! Credential pin (#521).
use super::step_tenant;
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};

#[rstest::rstest]
fn a_tenant_scoped_step_names_its_tenant() {
	let scope = TenancyScope::tenant("frosty-lynx-lo41ipfw").unwrap();
	assert_eq!(step_tenant(Some(&scope)), "frosty-lynx-lo41ipfw");
}

#[rstest::rstest]
#[case::no_authority(None)]
#[case::no_tenant(Some(TenancyScope::NoTenant))]
#[case::deployment(Some(TenancyScope::Deployment(DeploymentScopeReason::ActivationClaim)))]
fn a_step_without_a_tenant_names_none(#[case] scope: Option<TenancyScope>) {
	assert_eq!(step_tenant(scope.as_ref()), "");
}
