#!/usr/bin/env python3
"""Plan/apply reviewed delivery stacks and install secrets after live protection checks.

Credentials use process environment/stdin. Private plans/logs stay in the ignored
private input directory; only resource addresses/actions and protection status print.
This tool cannot merge PRs, approve deployments or enable workflows implicitly.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[3]
REPOSITORY = "kent8192/aidash"
ENVIRONMENTS = {
    "nonproduction": "develop/0.1.0", "production": "main",
    "release-automation": "develop/0.1.0", "deployment-notifications": "develop/0.1.0",
    "certificate-renewal": "develop/0.1.0",
}
RULESETS = {"main": "Aidash delivery main checks", "develop": "Aidash delivery develop checks"}
INITIAL_VARIABLES = {"GCP_ENVIRONMENTS_ENABLED", "GCP_PRODUCTION_ENABLED", "GCP_DEPLOYMENT_NOTIFICATIONS_ENABLED", "GCP_CERTIFICATE_RENEWAL_ENABLED"}
SECRETS = {
    "nonproduction": {"CLOUDFLARE_DNS_API_TOKEN", "GCP_DEVELOP_RUNTIME_CONFIG", "GCP_TEST_RUNTIME_CONFIG", "GCP_PR_RUNTIME_CONFIG", "GCP_GCIP_IDP_SECRETS"},
    "production": {"CLOUDFLARE_DNS_API_TOKEN", "GCP_PRODUCTION_RUNTIME_CONFIG", "GCP_PRODUCTION_GCIP_IDP_SECRETS"},
    "release-automation": {"RELEASE_APP_PRIVATE_KEY"},
    "deployment-notifications": {"CLOUDFLARE_EMAIL_SENDING_TOKEN"},
    "certificate-renewal": {"CLOUDFLARE_CERTIFICATE_DNS_TOKEN"},
}
BROKER_SETTINGS = {"enabled", "signing_version", "verification_versions"}


class Refused(ValueError):
    """Safe public diagnostics with no raw command output or supplied input."""


def private_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if path.is_symlink():
        raise Refused("Private input files must not be symlinks")
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as output:
        os.chmod(path, 0o600)
        json.dump(value, output, indent=2, sort_keys=True)
        output.write("\n")


def command(arguments, *, env=None, data=None, log=None):
    environment = dict(os.environ if env is None else env)
    for name in ("TF_LOG", "TF_LOG_PATH", "TF_LOG_CORE", "TF_LOG_PROVIDER"):
        environment.pop(name, None)
    try:
        result = subprocess.run(arguments, input=data, capture_output=True, env=environment, timeout=1800)
    except (OSError, subprocess.SubprocessError):
        raise Refused("Setup command unavailable or timed out; inspect authenticated CLI status") from None
    if log:
        path = Path(log)
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, "ab") as output:
            raw = result.stdout + result.stderr
            for name, value in environment.items():
                if value and any(part in name.upper() for part in ("TOKEN", "CREDENTIAL", "PRIVATE_KEY")):
                    raw = raw.replace(value.encode(), b"[redacted]")
            output.write(raw)
    if result.returncode:
        raise Refused("Setup command failed; inspect its private log or authenticated CLI status")
    return result.stdout


def github(path):
    return json.loads(command(["gh", "api", path]))


def pages(path, field=None):
    result = []
    for page in range(1, 101):
        separator = "&" if "?" in path else "?"
        response = github(f"{path}{separator}per_page=100&page={page}")
        items = response[field] if field else response
        result.extend(items)
        if len(items) < 100:
            return result
    raise Refused("The delivery inventory exceeds its bounded pagination limit")


def github_adoption(managed_variables=INITIAL_VARIABLES):
    repo = github(f"repos/{REPOSITORY}")
    if repo.get("default_branch") != "develop/0.1.0" or repo.get("full_name") != REPOSITORY:
        raise Refused("The repository default/execution branch differs from the approved delivery setup")
    found = pages(f"repos/{REPOSITORY}/rulesets")
    rules = {}
    for identity, name in RULESETS.items():
        matches = [item for item in found if item.get("name") == name]
        if len(matches) > 1:
            raise Refused("The owned delivery ruleset name is ambiguous")
        if matches:
            detail = github(f"repos/{REPOSITORY}/rulesets/{matches[0]['id']}")
            conditions = detail.get("conditions", {}).get("ref_name", {})
            checks = detail.get("rules", [])
            if (detail.get("name") != name or detail.get("target") != "branch" or detail.get("enforcement") != "active"
                    or detail.get("bypass_actors") or conditions.get("include") != ["refs/heads/" + ("main" if identity == "main" else "develop/0.1.0")]
                    or conditions.get("exclude") != [] or len(checks) != 1 or checks[0].get("type") != "required_status_checks"
                    or any(item.get("context") not in {"CI Success", "Staging deployment"} or type(item.get("integration_id")) is not int
                           for item in checks[0].get("parameters", {}).get("required_status_checks", []))):
                raise Refused("An owned ruleset has additional controls; review before adopting it")
            rules[identity] = matches[0]["id"]
    environments = pages(f"repos/{REPOSITORY}/environments", "environments")
    retained = {item["name"] for item in environments if item["name"] in ENVIRONMENTS}
    policies = {}
    for name in retained:
        owned = pages(f"repos/{REPOSITORY}/environments/{name}/deployment-branch-policies", "branch_policies")
        if len(owned) != 1 or owned[0].get("name") != ENVIRONMENTS[name] or owned[0].get("type", "branch") != "branch":
            raise Refused("An existing delivery Environment has unowned or non-exact branch policies; review before adoption")
        policies[name] = owned[0]["id"]
    variables = pages(f"repos/{REPOSITORY}/actions/variables", "variables")
    existing = {item["name"] for item in variables} & set(managed_variables)
    if any(item["name"] in INITIAL_VARIABLES and item["value"] != "false" for item in variables):
        raise Refused("A workflow is already enabled; setup must preserve an explicitly reviewed activation")
    return {"existing_delivery_ruleset_ids": rules, "existing_environments": sorted(retained),
            "existing_deployment_policy_ids": policies, "existing_repository_variables": sorted(existing)}


def verify_environment(name, expected_reviewers=None):
    if name not in ENVIRONMENTS:
        raise Refused("Secrets belong only in fixed delivery Environments")
    value = github(f"repos/{REPOSITORY}/environments/{name}")
    policy = value.get("deployment_branch_policy") or {}
    if policy.get("protected_branches") is not False or policy.get("custom_branch_policies") is not True or value.get("can_admins_bypass") is not False:
        raise Refused("The live Environment lacks strict custom branch protection or disables administrator bypass")
    branches = pages(f"repos/{REPOSITORY}/environments/{name}/deployment-branch-policies", "branch_policies")
    if len(branches) != 1 or branches[0].get("name") != ENVIRONMENTS[name] or branches[0].get("type", "branch") != "branch":
        raise Refused("The live Environment must permit only its one exact execution branch")
    if name == "production":
        rules = [rule for rule in value.get("protection_rules", []) if rule.get("type") == "required_reviewers"]
        reviewers = {item["reviewer"]["id"] for rule in rules for item in rule.get("reviewers", []) if item.get("type") == "User"}
        if len(rules) != 1 or rules[0].get("prevent_self_review") is not True or not reviewers or expected_reviewers is not None and reviewers != set(expected_reviewers):
            raise Refused("Production requires the declared human reviewers and prevention of self review")
    return {"environment": name, "branch": ENVIRONMENTS[name], "protected": True}


def install_secrets(values):
    if not isinstance(values, dict) or set(values) - set(SECRETS):
        raise Refused("Secret input contains an undeclared Environment")
    # Validate every destination before the first write; recheck at its boundary.
    for environment, entries in values.items():
        if not isinstance(entries, dict) or set(entries) - SECRETS[environment] or any(not isinstance(value, str) or not value or len(value.encode()) > 48000 for value in entries.values()):
            raise Refused("Secret input contains an undeclared or invalid protected value")
        verify_environment(environment)
    count = 0
    for environment, entries in values.items():
        for name, value in entries.items():
            verify_environment(environment)
            command(["gh", "secret", "set", name, "--repo", REPOSITORY, "--env", environment], data=value.encode())
            count += 1
    return {"protected_secrets_installed": count}


def authenticate():
    environment = dict(os.environ)
    environment["GITHUB_TOKEN"] = command(["gh", "auth", "token"]).decode().strip()
    environment["GOOGLE_OAUTH_ACCESS_TOKEN"] = command(["gcloud", "auth", "print-access-token", "--quiet"]).decode().strip()
    return environment


def private_input(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor) as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) & 0o077:
            raise Refused("Private inputs must be files owned by the current user")
        return json.load(source)


def compose_brokers(bootstrap, output, settings):
    """Carry bootstrap broker identities into controller intent.

    Omitting a bootstrap broker would make the controller drain and remove it, so
    every bootstrap-created broker is published; only enablement and signing
    versions come from your inputs, and every broker defaults to disabled.
    """
    accounts = bootstrap.get("broker_service_accounts") or {}
    keys = (output.get("broker_signing_keys") or {}).get("value") or {}
    if set(accounts) != set(keys) or not isinstance(settings, dict) or set(settings) - set(accounts):
        raise Refused("Credential broker settings must name bootstrap-created broker identities and signing keys")
    brokers = {}
    for identity in sorted(accounts):
        chosen = settings.get(identity, {})
        if not isinstance(chosen, dict) or set(chosen) - BROKER_SETTINGS or not isinstance(chosen.get("enabled", False), bool):
            raise Refused("Inputs may set only broker enablement and signing versions")
        brokers[identity] = dict({"enabled": False}, **chosen, byok_project_id=bootstrap["byok_project_id"], secret_prefix=f"aidash-{identity}-cred-",
                                 broker_service_account_email=accounts[identity], signing_key_id=keys[identity])
    return brokers


def compose(values):
    """Generate disabled account inputs from real bootstrap outputs and App metadata."""
    common = {"repository": REPOSITORY, "domain": "aidash.run", "develop_branch": "develop/0.1.0",
              "automation_branch": "develop/0.1.0", "trusted_branch": "main", "cloudflare_zone_id": values["cloudflare_zone_id"],
              "release_flow_enabled": True}
    release = private_input(values["release_app_file"])["release_app"]
    common["release_app_bot_login"] = release["slug"] + "[bot]"
    result = {"configure_variables": True, "release_app": release,
              "production_reviewer_user_ids": values["production_reviewer_user_ids"],
              "cloudflare_account_id": values.get("cloudflare_account_id", ""), "acme_email": values.get("acme_email", "")}
    controller_keys = {"project_id", "deployment_tier", "state_bucket", "release_bucket", "notification_bucket", "notification_service_account",
                       "deploy_service_account", "byok_project_id", "nonproduction_project_id", "nonproduction_state_bucket", "staging_project_id", "staging_release_bucket"}
    for tier in ("nonproduction", "production"):
        output = private_input(values[tier + "_output_file"])
        bootstrap = output["configuration"]["value"]
        if bootstrap.get("deployment_tier") != tier:
            raise Refused("The bootstrap output does not match its declared tier")
        public = {key: bootstrap[key] for key in controller_keys if key in bootstrap and bootstrap[key] not in (None, "")}
        public.update(common, node_count={kind: 1 for kind in (["production"] if tier == "production" else ["develop", "test", "pr"])},
                      gcip_tenants=values["gcip_tenants"][tier])
        brokers = compose_brokers(bootstrap, output, values.get("credential_brokers", {}).get(tier, {}))
        if brokers:
            public["credential_brokers"] = brokers
        if not isinstance(public["gcip_tenants"], dict) or not public["gcip_tenants"]:
            raise Refused("Random preview and production sign-in require explicit GCIP pool definitions")
        # The browser SDK key is public client configuration (Terraform only marks
        # the output sensitive); the controller injects it into the runtime secret.
        api_key = (output.get("gcip_web_api_key") or {}).get("value")
        if not isinstance(api_key, str) or not api_key.strip():
            raise Refused("GCIP sign-in requires the bootstrap gcip_web_api_key output")
        public["gcip_web_api_key"] = api_key
        if tier == "nonproduction":
            renewal = output["preview_tls_renewal"]["value"]
            public.update(preview_tls_secret=renewal["secret_id"], acme_state_secret=renewal["acme_state_secret_id"])
            bootstrap = dict(bootstrap, preview_tls_renewal=renewal)
        else:
            public.update(production_origin="https://aidash.run", gke_backup_enabled=output["operations_configuration"]["value"]["gke_backup_enabled"])
        result[tier + "_bootstrap"] = bootstrap
        result[tier + "_config_json"] = json.dumps(public, separators=(",", ":"), sort_keys=True)
    managed = INITIAL_VARIABLES | {"AIDASH_GCP_CONFIG", "AIDASH_GCP_PRODUCTION_CONFIG", "GCP_WORKLOAD_IDENTITY_PROVIDER", "GCP_DEPLOY_SERVICE_ACCOUNT",
        "GCP_PUBLISH_SERVICE_ACCOUNT", "GCP_NOTIFICATION_SERVICE_ACCOUNT", "GCP_PRODUCTION_WORKLOAD_IDENTITY_PROVIDER", "GCP_PRODUCTION_DEPLOY_SERVICE_ACCOUNT",
        "GCP_PRODUCTION_NOTIFICATION_SERVICE_ACCOUNT", "RELEASE_APP_CLIENT_ID", "GCP_TLS_RENEWAL_WORKLOAD_IDENTITY_PROVIDER", "GCP_TLS_RENEWAL_SERVICE_ACCOUNT"}
    if result["acme_email"]:
        managed.add("AIDASH_ACME_EMAIL")
    result.update(github_adoption(managed))
    if result["cloudflare_account_id"]:
        variables = pages(f"repos/{REPOSITORY}/environments/deployment-notifications/variables", "variables")
        result["existing_notification_account_variable"] = any(item["name"] == "CLOUDFLARE_ACCOUNT_ID" for item in variables)
    return result


def project_repair_allowed(values, state, project, billing, network, firewalls, instances):
    """Finish only the recorded new project's interrupted default-network cleanup."""
    identity = values["production_project_id"]
    retained = [instance for resource in state.get("resources", []) if resource.get("type") == "google_project" and resource.get("name") == "production"
                for instance in resource.get("instances", [])]
    if (len(retained) != 1 or retained[0].get("status") != "tainted" or retained[0].get("attributes", {}).get("project_id") != identity
            or project.get("projectId") != identity or project.get("state") != "ACTIVE"
            or project.get("labels", {}).get("application") != "aidash" or project.get("labels", {}).get("environment") != "production"
            or billing.get("billingAccountName") != "billingAccounts/" + values["billing_account_id"] or billing.get("billingEnabled") is not True
            or instances.get("nextPageToken") or any(item.get("instances") for item in instances.get("items", {}).values())):
        raise Refused("Project cleanup requires the recorded new isolated project, correct billing and no workloads")
    if network is None:
        return []
    if network.get("name") != "default" or network.get("autoCreateSubnetworks") is not True or network.get("description") != "Default network for the project" or network.get("peerings"):
        raise Refused("Refusing cleanup of a customized or connected network")
    if firewalls.get("nextPageToken"):
        raise Refused("Default network firewall inventory must be complete")
    names = []
    for rule in firewalls.get("items", []):
        if rule.get("network") != network.get("selfLink"):
            continue
        if rule.get("name") not in {"default-allow-ssh", "default-allow-rdp", "default-allow-icmp", "default-allow-internal"}:
            raise Refused("The default network contains an unowned firewall rule")
        names.append(rule["name"])
    return names


def repair_project(stack, values):
    if stack.name != "projects":
        raise Refused("Project repair applies only to the explicit project creation stack")
    identity = values["production_project_id"]
    if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", identity):
        raise Refused("Invalid isolated production project ID")
    token = stack.environment["GOOGLE_OAUTH_ACCESS_TOKEN"]
    def read(url, missing=False):
        request = urllib.request.Request(url, headers={"Authorization": "Bearer " + token})
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            if missing and error.code == 404:
                return None
            raise Refused("Project recovery API check failed; no resource was retired") from None
    state = json.loads(stack.run("state", "pull"))
    project = read("https://cloudresourcemanager.googleapis.com/v3/projects/" + identity)
    billing = read("https://cloudbilling.googleapis.com/v1/projects/" + identity + "/billingInfo")
    base = "https://compute.googleapis.com/compute/v1/projects/" + identity
    network = read(base + "/global/networks/default", missing=True)
    firewalls = read(base + "/global/firewalls")
    instances = read(base + "/aggregated/instances")
    names = project_repair_allowed(values, state, project, billing, network, firewalls, instances)
    for name in names:
        command(["gcloud", "compute", "firewall-rules", "delete", name, "--project", identity, "--quiet"], env=stack.environment, log=stack.log)
    if network is not None:
        command(["gcloud", "compute", "networks", "delete", "default", "--project", identity, "--quiet"], env=stack.environment, log=stack.log)
    if read(base + "/global/networks/default", missing=True) is not None:
        raise Refused("The interrupted project's unused default network is still present")
    stack.run("untaint", "google_project.production")
    return {"production_project_id": identity, "default_network_absent": True, "state_repaired": True}


def reviewed_bootstrap_migration(item, project):
    """Only retire this Aidash identity's legacy VM grants or narrow its WIF binding."""
    change = item["change"]
    before, after = change.get("before") or {}, change.get("after") or {}
    address, actions = item["address"], change["actions"]
    legacy_roles = {"roles/compute.osAdminLogin", "roles/compute.securityAdmin", "roles/iap.tunnelResourceAccessor"}
    if item.get("type") == "google_project_iam_member" and actions == ["delete"]:
        role = before.get("role")
        return (role in legacy_roles and address == f'google_project_iam_member.deploy["{role}"]'
                and before.get("project") == project and before.get("member") == f"serviceAccount:aidash-deploy@{project}.iam.gserviceaccount.com")
    if item.get("type") == "google_project_service" and actions == ["delete"]:
        service = before.get("service")
        return (service in {"iap.googleapis.com", "oslogin.googleapis.com"} and address == f'google_project_service.required["{service}"]'
                and before.get("project") == project and before.get("disable_on_destroy") is False)
    match = re.fullmatch(r'google_service_account_iam_member\.github\["(deploy|publish)"\]', address)
    if item.get("type") == "google_service_account_iam_member" and match and set(actions) == {"create", "delete"}:
        account = f"projects/{project}/serviceAccounts/aidash-{match[1]}@{project}.iam.gserviceaccount.com"
        old_member = before.get("member", "")
        return (before.get("service_account_id") == after.get("service_account_id") == account
                and before.get("role") == after.get("role") == "roles/iam.workloadIdentityUser"
                and re.fullmatch(r"principalSet://iam.googleapis.com/projects/[0-9]+/locations/global/workloadIdentityPools/aidash-github/attribute.repository_id/1378229915", old_member)
                and old_member.removesuffix("/attribute.repository_id/1378229915") + "/attribute.workflow_role/deploy" == after.get("member"))
    return False


def plan_changes(value, bootstrap_project=None):
    changes = []
    for item in value.get("resource_changes", []):
        if item.get("mode") == "data":
            continue
        actions = item["change"]["actions"]
        if "delete" in actions and not (bootstrap_project and reviewed_bootstrap_migration(item, bootstrap_project)):
            raise Refused("The setup plan would delete or replace a resource; inspect and resolve adoption first")
        if actions != ["no-op"]:
            changes.append({"address": item["address"], "actions": actions})
    return changes


def relocate_production_state(stack, target_bucket):
    """Move only the just-bootstrapped production state into its own bucket."""
    if stack.name != "production" or not re.fullmatch(r"[a-z0-9][a-z0-9._-]{1,221}[a-z0-9]", target_bucket):
        raise Refused("State relocation requires the isolated production bootstrap and its explicit bucket")
    stack.initialize()
    before = json.loads(stack.run("state", "pull"))
    config = before.get("outputs", {}).get("configuration", {}).get("value", {})
    if (config.get("deployment_tier") != "production" or config.get("state_bucket") != target_bucket
            or config.get("project_id") == config.get("nonproduction_project_id") or stack.bucket == target_bucket):
        raise Refused("State relocation must target this production bootstrap's own isolated private bucket")
    token = stack.environment["GOOGLE_OAUTH_ACCESS_TOKEN"]
    object_name = urllib.parse.quote(stack.prefix + "/default.tfstate", safe="")
    def metadata(bucket, missing=False):
        request = urllib.request.Request(f"https://storage.googleapis.com/storage/v1/b/{bucket}/o/{object_name}", headers={"Authorization": "Bearer " + token})
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            if missing and error.code == 404:
                return None
            raise Refused("State relocation could not verify the exact backend object") from None
    source = metadata(stack.bucket)
    if metadata(target_bucket, missing=True) is not None:
        raise Refused("The production backend already has state; review adoption instead of overwriting it")
    stack.run("init", "-migrate-state", "-force-copy", "-input=false", "-no-color",
              "-backend-config=bucket=" + target_bucket, "-backend-config=prefix=" + stack.prefix)
    after = json.loads(stack.run("state", "pull"))
    if before.get("lineage") != after.get("lineage") or before.get("resources") != after.get("resources") or before.get("outputs") != after.get("outputs"):
        raise Refused("Production backend verification failed; preserve both copies for manual recovery")
    # Retire only the copied source object's exact generation. Versioned private
    # history remains retained; no bucket, project or unrelated object is deleted.
    request = urllib.request.Request(f"https://storage.googleapis.com/storage/v1/b/{stack.bucket}/o/{object_name}?ifGenerationMatch={int(source['generation'])}",
                                     headers={"Authorization": "Bearer " + token}, method="DELETE")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            response.read()
    except urllib.error.HTTPError:
        raise Refused("Production state moved; review the old exact source object before using either backend again") from None
    private_json(stack.directory / "backend-relocation.json", {"source_bucket": stack.bucket, "target_bucket": target_bucket, "prefix": stack.prefix, "lineage": after["lineage"]})
    for path in (stack.plan, stack.directory / "reviewed.json"):
        path.unlink(missing_ok=True)
    return {"production_backend": target_bucket, "prefix": stack.prefix, "lineage_preserved": True}


class Stack:
    def __init__(self, name, operator_directory, bucket, prefix=None, review_bootstrap_migration=False):
        sources = {"github": ROOT / "infra/delivery/github", "cloudflare": ROOT / "infra/delivery/cloudflare",
                   "projects": ROOT / "infra/delivery/projects", "nonproduction": ROOT / "infra/gcp/bootstrap", "production": ROOT / "infra/gcp/bootstrap"}
        if name not in sources or not re.fullmatch(r"[a-z0-9][a-z0-9._-]{1,221}[a-z0-9]", bucket):
            raise Refused("Use one fixed stack and explicit private state bucket")
        self.name, self.bucket = name, bucket
        self.review_bootstrap_migration = review_bootstrap_migration
        if review_bootstrap_migration and name != "nonproduction":
            raise Refused("Legacy bootstrap migration applies only to reviewed nonproduction state")
        self.prefix = prefix or "terraform/delivery/" + name
        if self.prefix not in {"terraform/bootstrap", "terraform/delivery/" + name}:
            raise Refused("The state prefix differs from this fixed delivery stack")
        self.source = sources[name]
        self.directory = Path(operator_directory).resolve() / name
        self.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(self.directory, 0o700)
        self.module = self.directory / "module"
        self.module.mkdir(exist_ok=True, mode=0o700)
        for path in self.module.glob("*.tf"):
            path.unlink()
        for path in list(self.source.glob("*.tf")) + list(self.source.glob(".terraform.lock.hcl")):
            shutil.copy2(path, self.module / path.name)
        if not any('backend "gcs"' in path.read_text() for path in self.module.glob("*.tf")):
            (self.module / "backend.tf").write_text('terraform {\n  backend "gcs" {}\n}\n')
        self.environment = authenticate()
        self.environment["TF_DATA_DIR"] = str(self.directory / "data")
        self.environment["TF_IN_AUTOMATION"] = "true"
        self.plan = self.directory / "reviewed.tfplan"
        self.log = self.directory / "terraform.log"

    def run(self, *arguments):
        return command(["terraform", "-chdir=" + str(self.module), *arguments], env=self.environment, log=self.log)

    def initialize(self):
        self.run("init", "-reconfigure", "-input=false", "-no-color", "-backend-config=bucket=" + self.bucket, "-backend-config=prefix=" + self.prefix)

    def create_plan(self, values):
        variables = self.directory / "operator.tfvars.json"
        private_json(variables, values)
        if self.name == "cloudflare" and values.get("cloudflare_api_token"):
            self.environment["TF_VAR_cloudflare_api_token"] = values["cloudflare_api_token"]
        self.initialize()
        self.run("plan", "-input=false", "-no-color", "-var-file=" + str(variables), "-out=" + str(self.plan))
        os.chmod(self.plan, 0o600)
        value = json.loads(self.run("show", "-json", str(self.plan)))
        migration = values.get("project_id") if self.review_bootstrap_migration and values.get("deployment_tier") == "nonproduction" else None
        changes = plan_changes(value, migration)
        receipt = {"protocol": "aidash-delivery-plan/1", "stack": self.name, "bucket": self.bucket, "prefix": self.prefix,
                   "plan_sha256": hashlib.sha256(self.plan.read_bytes()).hexdigest(), "changes": changes, "bootstrap_migration_project": migration}
        private_json(self.directory / "reviewed.json", receipt)
        return receipt

    def apply(self):
        receipt = json.loads((self.directory / "reviewed.json").read_text())
        if (receipt.get("protocol") != "aidash-delivery-plan/1" or receipt.get("stack") != self.name or receipt.get("bucket") != self.bucket
                or receipt.get("prefix") != self.prefix or receipt.get("plan_sha256") != hashlib.sha256(self.plan.read_bytes()).hexdigest()):
            raise Refused("Saved plan differs from the exact reviewed stack/backend")
        if self.name == "cloudflare":
            token = private_input(self.directory / "operator.tfvars.json").get("cloudflare_api_token")
            if token:
                # Ephemeral inputs are absent from the saved plan and must be
                # supplied again at apply. No planning inputs can override it.
                self.environment["TF_VAR_cloudflare_api_token"] = token
        self.initialize()
        migration = receipt.get("bootstrap_migration_project")
        if migration and not self.review_bootstrap_migration:
            raise Refused("This saved plan includes a legacy bootstrap migration that needs its explicit review flag")
        plan_changes(json.loads(self.run("show", "-json", str(self.plan))), migration)
        self.run("apply", "-input=false", "-no-color", str(self.plan))
        result = json.loads(self.run("output", "-json"))
        private_json(self.directory / "outputs.json", result)
        return {"stack": self.name, "applied": True, "outputs_file": str(self.directory / "outputs.json")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["inventory", "plan", "apply", "verify", "secrets", "repair-project", "compose", "relocate-production-state"])
    parser.add_argument("--stack", choices=["github", "cloudflare", "projects", "nonproduction", "production"])
    parser.add_argument("--file", type=Path)
    parser.add_argument("--state-bucket")
    parser.add_argument("--target-state-bucket")
    parser.add_argument("--prefix")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--review-bootstrap-migration", action="store_true")
    parser.add_argument("--operator-directory", type=Path, default=ROOT / "secrets/delivery/operator")
    args = parser.parse_args()
    try:
        if args.action == "inventory":
            result = github_adoption()
        elif args.action == "verify":
            result = {"environments": [verify_environment(name) for name in ENVIRONMENTS]}
        elif args.action == "secrets":
            result = install_secrets(private_input(args.file))
        elif args.action == "compose":
            private_json(args.output, compose(private_input(args.file)))
            result = {"disabled_account_inputs": str(args.output)}
        else:
            stack = Stack(args.stack, args.operator_directory, args.state_bucket or "", args.prefix, args.review_bootstrap_migration)
            if args.action == "repair-project":
                result = repair_project(stack, private_input(args.file))
            elif args.action == "relocate-production-state":
                result = relocate_production_state(stack, args.target_state_bucket or "")
            else:
                result = stack.create_plan(private_input(args.file)) if args.action == "plan" else stack.apply()
        print(json.dumps(result, sort_keys=True))
    except (Refused, OSError, ValueError, KeyError, TypeError) as error:
        print(str(error) if isinstance(error, Refused) else "Setup failed; review private inputs and logs")
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
