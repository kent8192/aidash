"""Disposable execution settings shared by the manual Kubernetes fixtures."""
import json
import secrets


def fixture_runtime(apply, name):
    """Provision fixture-owned storage and keys, and return their Pod settings.

    Native core admission and File Tools are needed by the Agents' default
    bindings. No command runner or sandbox execution is enabled here.
    """
    root = "/etc/aidash/acceptance"
    storage = "/var/lib/aidash/acceptance"
    keys = ("AIDASH_PROVIDER_FINGERPRINT_KEY", "AIDASH_PROVIDER_STORE_MASTER_KEY")
    apply({"apiVersion": "v1", "kind": "Secret", "metadata": {"name": name},
           "stringData": {key: secrets.token_hex(32) for key in keys}})
    apply({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": name}, "data": {
        "profile.json": json.dumps({"admission": True, "storage": storage + "/objects"}),
        "provider-credentials.json": json.dumps({"provider_credentials": {
            "fingerprint_key": {"env": keys[0]},
            "store": {"kind": "postgres", "master_key": {"env": keys[1]}},
        }}),
    }})
    apply({"apiVersion": "v1", "kind": "PersistentVolumeClaim", "metadata": {"name": name},
           "spec": {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": "1Gi"}}}})
    return {
        "securityContext": {"runAsUser": 10001, "runAsGroup": 10001, "fsGroup": 10001},
        "volumes": [
            {"name": "acceptance-config", "configMap": {"name": name}},
            {"name": "acceptance-storage", "persistentVolumeClaim": {"claimName": name}},
        ],
        "containers": [{"name": "aidash", "env": [
            {"name": "AIDASH_CAPABILITY_PROFILE", "value": root + "/profile.json"},
            {"name": "AIDASH_PROVIDER_CREDENTIAL_SETTINGS", "value": root + "/provider-credentials.json"},
            *({"name": key, "valueFrom": {"secretKeyRef": {"name": name, "key": key}}} for key in keys),
        ], "volumeMounts": [
            {"name": "acceptance-config", "mountPath": root, "readOnly": True},
            {"name": "acceptance-storage", "mountPath": storage},
        ]}],
    }
