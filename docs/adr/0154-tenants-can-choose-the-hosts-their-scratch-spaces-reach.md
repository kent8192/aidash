# 0154. Tenants can choose the hosts their Scratch Spaces reach

Decided on 2026-11-28 in the Tenant Configurator live check (Issue #1155, ADR 0151).

The Tenant Configurator and the Agents it sets up must reach the services a person's request needs. During the live check, the Configurator could configure a Scratch Space but needed a Deployment Administrator to admit ordinary service hosts, including `api.x.com` and `upload.x.com`. The new space's package registry defaults had also become the Tenant's policy ceiling.

New Scratch Spaces start with the common public package, source, and container registries. Permissions to manage the space include choosing its hosts. By default a Tenant can add any valid host or choose all hosts. The Deployment can set an explicit host list for a Tenant or turn off the all-hosts setting across the Deployment. The limits response and settings validation use the same decision for whether the Tenant may select all hosts.

The Egress gateway checks the effective Scratch Space settings, the Run's scope, and any remote Home ceiling. The gateway's public destination and port checks still apply. The separate `outbound_get` capability continues to use the Deployment's `outbound_origins` and its own approval flow.
