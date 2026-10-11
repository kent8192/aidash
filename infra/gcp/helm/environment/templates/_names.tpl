{{/*
Every generated name is "<release>-environment-<component>"; the longest
components (postgres, activity) add 21 characters. Services must fit a
63-character DNS label, and the postgres/nats StatefulSets and the activity
CronJob must also leave 11 characters for their controller-generated
suffixes, so names stay within 52 characters and releases within 31.
*/}}
{{- define "environment.name" -}}
{{- if gt (len .Release.Name) 31 -}}{{ fail "release name is too long: Environment StatefulSet, CronJob and Service names need a release name of at most 31 characters" }}{{- end -}}
{{- printf "%s-environment" .Release.Name -}}
{{- end -}}
{{/*
Egress policy denying the GCE/GKE metadata server (169.254.169.254) to every Pod
that lacks the aidash.run/metadata-server label. Keep in step with
aidash.metadataEgressPolicy in deploy/helm/aidash/templates/_network.tpl.
*/}}
{{- define "environment.metadataEgressPolicy" -}}
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: {{ .name }}
spec:
  podSelector:
    matchExpressions:
      - { key: aidash.run/metadata-server, operator: DoesNotExist }
  policyTypes: [Egress]
  egress:
    - to:
        - namespaceSelector: {}
    - to:
        - ipBlock:
            cidr: 0.0.0.0/0
            except: [169.254.169.254/32]
{{- end -}}
