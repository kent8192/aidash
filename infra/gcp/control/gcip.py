"""Tenant IAM reconciliation; never grants a runtime principal project-wide access."""
from copy import deepcopy
import json
import re
import urllib.error
import urllib.request
from cloud import run, bounded_timeout

# Contains firebaseauth.users.get. Tenant-scope enforcement is a sandbox assumption.
READ_ROLE = "roles/identityplatform.viewer"


class TenantIAM:
    def call(self, resource, method, body):
        token = run("gcloud", "auth", "print-access-token", timeout=30).decode().strip()
        request = urllib.request.Request(
            f"https://identitytoolkit.googleapis.com/admin/v2/{resource}:{method}",
            data=json.dumps(body).encode(), method="POST",
            headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=bounded_timeout(60)) as response:
            return json.load(response)

    def reconcile(self, project, tenant, service_account, enabled=True):
        if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", project) or not re.fullmatch(r"[A-Za-z0-9_-]+", tenant):
            raise ValueError("Invalid GCIP project or tenant")
        if not re.fullmatch(r"[a-zA-Z0-9_-]+@[a-z0-9-]+\.iam\.gserviceaccount\.com", service_account):
            raise ValueError("Invalid runtime service account")
        resource = f"projects/{project}/tenants/{tenant}"
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


def reconcile_environment(output, enabled=True, api=None):
    config = (output or {}).get("gcip", {})
    tenants = config.get("tenant_ids", [])
    if not tenants:
        return
    api = api or TenantIAM()
    for tenant in tenants:
        api.reconcile(config["project_id"], tenant, config["runtime_service_account"], enabled)
