"""Tenant IAM and MFA reconciliation; never grants a runtime principal project-wide access."""
from copy import deepcopy
import json
import re
import urllib.error
import urllib.request
from cloud import run, bounded_timeout

# Contains firebaseauth.users.get. Tenant-scope enforcement is a sandbox assumption.
READ_ROLE = "roles/identityplatform.viewer"
MFA_STATES = ("disabled", "enabled", "mandatory")
# GCIP accepts 0-10 adjacent 30-second steps around the current TOTP code.
TOTP_ADJACENT_INTERVALS = range(11)
PREVIEW_HOST = re.compile(r"[a-f0-9]{32}\.preview\.aidash\.run")


def declared_mfa_config(declared):
    """The complete tenant mfaConfig GCIP must hold for a declared MFA Requirement.

    `declared` is the module output {"state": ..., "totp_adjacent_intervals": ...}.
    TOTP is the only second factor; `enabledProviders` lists SMS factors only, so
    it stays empty and TOTP is enabled through its provider config.
    """
    state = declared.get("state") if isinstance(declared, dict) else None
    if state not in MFA_STATES:
        raise ValueError("Unsupported MFA Requirement")
    if state == "disabled":
        return {"state": "DISABLED", "enabledProviders": [], "providerConfigs": []}
    intervals = declared.get("totp_adjacent_intervals")
    if isinstance(intervals, bool) or intervals not in TOTP_ADJACENT_INTERVALS:
        raise ValueError("Unsupported MFA Requirement")
    return {
        "state": state.upper(),
        "enabledProviders": [],
        "providerConfigs": [{"state": "ENABLED", "totpProviderConfig": {"adjacentIntervals": intervals}}],
    }


def ensure_authorized_domain(output, enabled=True, api=None):
    """Manage one exact preview origin while holding the controller lifecycle lock.

    Identity Platform's Config has no etag. The shared controller lock serializes
    our read/modify/write operations; unrelated domains are read on each call and
    only authorizedDomains is patched. Operators must use that same lock when
    changing this field. Fixed environments remain Terraform-managed.
    """
    hostname = (output or {}).get("hostname", "")
    if not hostname.endswith(".preview.aidash.run"):
        return
    if not PREVIEW_HOST.fullmatch(hostname):
        raise ValueError("Preview authentication requires one exact random hostname")
    config = output.get("gcip") or {}
    project = config.get("project_id", "")
    if (
        not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", project)
        or not config.get("tenant_ids")
        or config.get("auth_helper", "firebase") != "firebase"
    ):
        raise ValueError("Random preview authentication requires managed GCIP with the fixed Firebase helper")
    request = api or admin_request
    resource = f"https://identitytoolkit.googleapis.com/admin/v2/projects/{project}/config"
    actual = request(resource, "GET")
    domains = actual.get("authorizedDomains", [])
    if not isinstance(domains, list) or any(not isinstance(domain, str) for domain in domains):
        raise ValueError("Identity Platform returned invalid authorized domains")
    if (hostname in domains) == enabled:
        return
    desired = domains + [hostname] if enabled else [domain for domain in domains if domain != hostname]
    request(resource + "?updateMask=authorizedDomains", "PATCH", {"authorizedDomains": desired})


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
        desired = declared_mfa_config(requirement)
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
    """Enforce every declared MFA Requirement, yielding (tenant_id, drifted fields) per correction.

    Each correction is yielded before the next tenant is touched, so a later
    tenant's failure cannot hide drift already overwritten on an earlier one.
    """
    config = (output or {}).get("gcip", {})
    tenants = config.get("tenant_ids", [])
    # State written before tenant MFA existed declares nothing until its next apply.
    if not tenants or "mfa" not in config:
        return
    api = api or TenantMFA()
    for tenant in tenants:
        fields = api.reconcile(config["project_id"], tenant, config["mfa"][tenant])
        if fields:
            yield tenant, fields
