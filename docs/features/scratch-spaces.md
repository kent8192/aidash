# Scratch Spaces

Every Node has a persistent default Scratch Space. Threads, Duties and Subnodes share it. Runs executing for another Home use the receiving Node's default space. Anyone who may give work to the Node, and its administrators, may use the default space. Deployment Administrators have no Tenant authority to read spaces.

A person can create a named Scratch Space in the same Node and select it when opening a Thread. Members and the creator can use it. The creator may clear or delete it. The Thread keeps its selection and shows the space to people with access.

The default space starts with 8 CPU, 32 GiB memory, 500 GiB capacity and 32 foreground command slots. Named spaces start with 4 CPU, 16 GiB memory, 100 GiB capacity and 8 slots. Both start with daily egress budgets of 100 GiB each for sent and received traffic. At use time, resource ceilings cap those settings, and the Tenant's current host allowlist narrows the space's allowed hosts.

A space may choose a digest-pinned execution image approved for the Tenant and share approved External services through the Egress gateway. Docker requires permission from the Deployment operator and a Node dedicated to that Tenant.

Files persist while the Execution Pod is idle or stopped. A Node administrator may clear the default space after confirming the list of affected running Tasks. Clearing records a new wipe generation. The space remains active. Node and Tenant purge remove their spaces.

If a plain HTTP proxy request already has a Host connection record when the gateway returns a proxy error, the record includes that error code. The `egress_client_closed` refusal tally counts clients that disconnect while waiting for a connection limit. Waits cancelled by gateway shutdown are excluded from that tally.

## Operator configuration

The `[deployment]` settings `scratch_capacity_bytes`, `scratch_cpu_millis`, and `scratch_memory_bytes` set ceilings. The defaults are 1 TiB, 8,000 millicores and 32 GiB, respectively. `scratch_daily_sent_bytes` and `scratch_daily_received_bytes` set per-day egress ceilings, each defaulting to 1 TiB. `scratch_docker_allowed` enables Docker for the Offering, and `scratch_docker_tenants` lists additional Tenants for which Docker may be enabled. `scratch_all_hosts_allowed` controls whether the Offering permits choosing all hosts. `scratch_tenant_hosts` maps Tenant names to their permitted host lists; a one-element `['*']` list represents all hosts. Without a Tenant entry the public-registry defaults apply. `scratch_tenant_images` maps Tenant names to approved image references.

## HTTP API

`POST /api/threads` accepts an optional `scratch_space_id`; omission selects the Thread Node's default space. `GET` and `POST /api/scratch-spaces` list and create spaces. Creation accepts a Node, name, and optional settings; when settings are omitted the defaults are narrowed to the Tenant allowlist. `GET` and `DELETE /api/scratch-spaces/{id}` read and delete one. Settings are updated with `PUT /api/scratch-spaces/{id}/settings` using the last observed revision; stale updates return HTTP 409. Members are added with `POST /api/scratch-spaces/{id}/members` and removed with `POST /api/scratch-spaces/{id}/members/remove`; both accept a subject. `POST /api/scratch-spaces/{id}/clear` with an empty object lists affected running Tasks. Repeat it with `confirmed_tasks` set to the returned `running_tasks` to clear the space. A change to the Task list returns HTTP 409. `GET /api/scratch-spaces/limits?node=...` returns effective ceilings and the Tenant allowlist for the Node. Each operation requires Tenant authority and applies the space's membership and Node checks.
