# Operating the Egress gateway

With `execution.egress.enabled`, Helm starts one Egress gateway Deployment,
ClusterIP Service and autoscaler per Egress IP group. The API and gateway
run in separate Pods. Each gateway has a 2 GiB memory request and limit,
a database pool of eight connections, and a memory autoscaler targeting
70% with one to ten replicas. The API's memory limit is 2 GiB. The GCP
environment chart starts PostgreSQL with `max_connections=300`. Count
eight connections per replica across all groups when sizing PostgreSQL.

Standalone installations can run `manage egress-gateway` with the same
capability profile, database credentials and Runner token as the API. Set
`egress.in_process` to `false` for the API and set
`AIDASH_EGRESS_IP_GROUP=shared` for the gateway. Assignments live in
`egress.tenant_groups`, a map from Tenant ID to group. Set
`egress.memory_limit_bytes` to the process's memory budget. When omitted,
the gateway uses the container's memory limit when the platform exposes it.

Remove `proxy_global_tunnels` and `proxy_tenant_tunnels` from standalone
profiles before upgrading. Helm uses the Egress Services for its Execution Pod
proxy when `execution.egress.enabled` is set. External proxy installations
continue to configure `execution.pods.proxy` when Helm Egress is disabled.

## Pressure and metrics

Every second a replica samples file descriptors and RSS. Each resource enters
shortage above 80% of its budget and leaves below 70%. With open-file pressure
alone, a Tenant above the mean number of open connections per active Tenant
waits until its share is available or the shortage clears. Under memory
pressure, an over-share connection receives HTTP 503 with `egress_fair_share`.
The gateway counts these refusals by scope. It also releases over-share
waiters if memory pressure begins during their wait.

The Run's Egress waits show admitted Host connections from
`egress_scope_connections.wait_ms`. External service records include their
wait in admission time.

`AIDASH_METRICS_LISTEN` enables the standalone metrics listener. Helm binds it
on port 9090. Configure `execution.egress.metricsScrapeNamespaces` with the
namespaces of external Prometheus scrapers. Scrape each gateway replica for:

- `aidash_egress_open_connections{tenant}` and `aidash_egress_open_connections_total`
- `aidash_egress_waiting_connections` and `aidash_egress_fair_share_wait_seconds`
- `aidash_egress_resource_shortage` and `aidash_egress_resource_shortage_by_type{resource}`
- `aidash_egress_process_open_files`, `aidash_egress_process_rss_bytes` and `aidash_egress_borrowed_buffer_bytes`
- `aidash_egress_accept_emfile_total`

A hard descriptor limit below 65,536 logs a startup warning. Run
`manage diagnostics evaluation` in the gateway container and inspect
`egress_open_files` for the soft limit, hard limit and warning. Raise the
node runtime's `LimitNOFILE`, replace the gateway Pods, and verify the new
limits. An accept that encounters `EMFILE` is counted and retried after
100 ms. Idle relays return their buffers before waiting for input.

## Egress IP groups

Use operator settings to assign Tenant IDs. An unassigned Tenant uses
the shared gateway. Reserve a distinct ClusterIP for every group and leave
it in the cluster's static Service IP band. Example Helm values:

```yaml
execution:
  egress:
    enabled: true
    clusterIP: 10.48.0.200
    sharedNodePool: true
    ipGroups:
      dedicated:
        clusterIP: 10.48.0.201
        tenants:
          - acme
```

Set `sharedNodePool` when the shared group has its own node pool. The
gateway's group label and scheduling taint use `aidash.run/egress-ip-group`.
Each gateway authenticates the Tenant and checks its group before opening
an upstream connection.

On managed GCP, set `egress_ip_groups` in the environments Terraform root.
Each entry supplies `node_cidr`, `pod_cidr`, an optional `address_count`, and
`environments` keyed by environment ID. Give each environment entry a
`clusterIP` and its list of `tenants`. The reserved key `shared` accepts
environment entries with an empty Tenant list. The controller passes the
applied assignment to Helm. Keep a group's subnets disjoint from other
subnets and reserve enough addresses for its peak connection count.

After deployment, verify the gateway Pods' node pool, node subnet and Pod
IP range. From Execution Pods of a dedicated and an unassigned Tenant, use
an authorized HTTPS IP-echo endpoint and compare the results with the
Terraform output `egress_ip_groups` or the default NAT addresses. Check
both the cluster's IP masquerading configuration and the NAT subnet range
selection. Each group NAT covers the subnet's primary and secondary ranges
and uses dynamic port allocation from 1,024 to 65,536 ports per VM.
See [GKE additional subnets](https://cloud.google.com/kubernetes-engine/docs/how-to/multi-subnet-cluster)
and [Cloud NAT configuration](https://registry.terraform.io/providers/hashicorp/google/latest/docs/resources/compute_router_nat).

For brokered Tenant Secret requests, the Credential Broker opens the final
upstream connection. Upstreams see the broker's outbound address on that
path. When changing a Tenant's assignment, restart its running Execution
Pod processes so their next start receives the new proxy address.
