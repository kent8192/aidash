"""Tenant IAM and MFA reconciliation; never grants a runtime principal project-wide access."""
from copy import deepcopy
import json
import re
import urllib.error
import urllib.request
from cloud import run, bounded_timeout

# Contains firebaseauth.users.get. Tenant-scope enforcement is a sandbox assumption.
READ_ROLE = "roles/identityplatform.viewer"
# Declared MFA Requirement -> the complete tenant mfaConfig GCIP must hold.
MFA_CONFIGS = {"disabled": {"state": "DISABLED", "enabledProviders": [], "providerConfigs": []}}


def admin_request(url, method, body=None):
    token = run("gcloud", "auth", "print-access-token", timeout=30).decode().strip()
    headers = {"Authorization": "Bearer " + token}
    if body is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(
        url, data=None if body is None else json.dumps(body).encode(), method=method, headers=headers,
    )
    with urllib.request.urlopen(request, timeout=bounded_timeout(60)) as response:
        return json.load(response)


def tenant_resource(project, tenant):
    if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", project) or not re.fullmatch(r"[A-Za-z0-9_-]+", tenant):
        raise ValueError("Invalid GCIP project or tenant")
    return f"projects/{project}/tenants/{tenant}"


class TenantIAM:
    def call(self, resource, method, body):
        return admin_request(f"https://identitytoolkit.googleapis.com/admin/v2/{resource}:{method}", "POST", body)

    def reconcile(self, project, tenant, service_account, enabled=True):
        resource = tenant_resource(project, tenant)
        if not re.fullmatch(r"[a-zA-Z0-9_-]+@[a-z0-9-]+\.iam\.gserviceaccount\.com", service_account):
            raise ValueError("Invalid runtime service account")
        member = "serviceAccount:" + service_account
        for _ in range(5):
            try:
                policy = self.call(resource, "getIamPolicy", {"options": {"requestedPolicyVersion": 3}})
            except urllib.error.HTTPError as error:
                if error.code == 404 and not enabled:
                    return  # Already destroyed after an interrupted retirement.
                raise
            updated = deepcopy(policy)
            bindings = updated.setdefault("bindings", [])
            binding = next((binding for binding in bindings if binding.get("role") == READ_ROLE and not binding.get("condition")), None)
            if enabled:
                if binding is not None and member in binding.get("members", []):
                    return
                if binding is None:
                    binding = {"role": READ_ROLE, "members": []}
                    bindings.append(binding)
                binding.setdefault("members", []).append(member)
            else:
                changed = False
                for binding in list(bindings):
                    if binding.get("role") == READ_ROLE and not binding.get("condition") and member in binding.get("members", []):
                        binding["members"].remove(member)
                        if not binding["members"]:
                            bindings.remove(binding)
                        changed = True
                if not changed:
                    return
            # An etag is mandatory for an existing policy. Empty policies have
            # no etag and can be created by the initial setIamPolicy request.
            if policy.get("bindings") and not policy.get("etag"):
                raise RuntimeError("Refusing to replace a tenant IAM policy without its etag")
            try:
                self.call(resource, "setIamPolicy", {"policy": updated, "updateMask": "bindings,etag"})
                return
            except urllib.error.HTTPError as error:
                if error.code not in (409, 412):
                    raise
        raise RuntimeError("Tenant IAM changed repeatedly; retry reconciliation")


def normalize_mfa(config):
    """Effective tenant MFA settings. New tenants omit mfaConfig, which GCIP treats as disabled."""
    config = config or {}
    state = config.get("state") or "DISABLED"
    return {
        "state": "DISABLED" if state == "STATE_UNSPECIFIED" else state,
        "enabledProviders": sorted(set(config.get("enabledProviders", [])) - {"PROVIDER_UNSPECIFIED"}),
        # A provider config that is not enabled cannot challenge anyone.
        "providerConfigs": sorted(
            (provider for provider in config.get("providerConfigs", []) if provider.get("state") not in (None, "DISABLED", "MFA_STATE_UNSPECIFIED")),
            key=lambda provider: json.dumps(provider, sort_keys=True),
        ),
    }


class TenantMFA:
    """The Google provider has no tenant MFA arguments, so the controller owns mfaConfig."""

    def call(self, method, resource, body=None):
        return admin_request(f"https://identitytoolkit.googleapis.com/v2/{resource}", method, body)

    def reconcile(self, project, tenant, requirement):
        """Enforce the declared requirement and return the corrected (drifted) fields."""
        resource = tenant_resource(project, tenant)
        if requirement not in MFA_CONFIGS:
            raise ValueError("Unsupported MFA Requirement")
        desired = MFA_CONFIGS[requirement]
        actual = normalize_mfa(self.call("GET", resource).get("mfaConfig"))
        drifted = sorted(field for field in desired if actual[field] != desired[field])
        if drifted:
            self.call("PATCH", resource + "?updateMask=mfaConfig", {"mfaConfig": deepcopy(desired)})
        return drifted


def reconcile_environment(output, enabled=True, api=None):
    config = (output or {}).get("gcip", {})
    tenants = config.get("tenant_ids", [])
    if not tenants:
        return
    api = api or TenantIAM()
    for tenant in tenants:
        api.reconcile(config["project_id"], tenant, config["runtime_service_account"], enabled)


def reconcile_mfa(output, api=None):
    """Return {tenant_id: drifted fields} after enforcing every declared MFA Requirement."""
    config = (output or {}).get("gcip", {})
    tenants = config.get("tenant_ids", [])
    # State written before tenant MFA existed declares nothing until its next apply.
    if not tenants or "mfa" not in config:
        return {}
    api = api or TenantMFA()
    drift = {}
    for tenant in tenants:
        fields = api.reconcile(config["project_id"], tenant, config["mfa"][tenant])
        if fields:
            drift[tenant] = fields
    return drift
