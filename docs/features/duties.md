# Duties

A **Duty** is a standing responsibility a Node carries out for a person
([ADR 0142](../adr/0142-a-duty-acts-only-for-its-requester-and-an-event-is-input-never-a-requester.md),
[#1011](https://github.com/kent8192/aidash/issues/1011)). It has a mission, a
procedure, Duty Triggers and its own Thread, and one Agent carries it out. A
Duty absorbs the Schedules of #557: a time trigger is one kind of Duty Trigger.

A Duty has no authority of its own. It always acts as one person, its
**Requester**: every Run it starts has the Requester at the root of its
delegation chain, so their Approval Mode, Approval Rules, Plan Connections and
MCP OAuth Connections apply exactly as for a Run they start by hand (ADR 0134).
A Duty whose Requester leaves stops until a named successor takes it over.

## Setting one up

A person sets up a Duty in a Workspace's **Duties** tab, or through
`POST /api/workspaces/{workspace}/duties`. It needs `workspace.read`,
`duty.create` and `task.create` on the Workspace; members hold all three by
default.

```json
{
  "title": "Daily report",
  "mission": "Summarize yesterday's merged pull requests.",
  "procedure": "1. Read every merged pull request.\n2. Group them by area.",
  "agent": { "id": "reporter", "version": "1.0.0" },
  "trigger": {
    "kind": "cron",
    "expression": "0 9 * * 1-5",
    "time_zone": "Asia/Tokyo"
  },
  "approval_mode": "dont_ask",
  "max_lateness_seconds": 3600,
  "pause_after_failures": 3
}
```

- `mission` is what every Task the Duty starts is asked. `procedure` is the
  checklist its Agent follows on every Task.
- `trigger` is the time trigger, if the Duty has one: a five-field cron
  expression (minute, hour, day of month, month, day of week, where Sunday is
  0 or 7) in an IANA time zone, or `{"kind":"interval","every_seconds":N}`
  counted from registration or resume. There is no minimum interval and no
  count limit (#1012): a Budget, not a count, bounds what a Duty spends. On
  Cloud a Tier may cap how many Duties a Tenant has. A small hidden jitter
  delays each tick by up to 30 seconds. A time a daylight-saving change skips
  runs once at the first instant after the gap; a repeated time runs once.
  An edit removes the time trigger with `{"kind":"none"}`.
- `approval_mode` is the mode of the Duty's Runs, within what the Tenant's
  policy lets the Requester use. Without it, their own default applies.
- `pause_after_failures` is how many failed Tasks in a row pause the Duty: 3
  by default, 0 never pauses it.
- The Agent version is pinned. The Duty's page shows when a newer version
  exists; the Requester moves it with an edit.

The Requester edits, pauses, resumes, deletes or runs a Duty now from the
Duties tab or **Settings → Duties** ("my Duties"). "Run now" starts an
ordinary Run as the person who clicked it, in their own Approval Mode, and
records it in the history.

A Duty runs on its Requester's Mapping credential, whichever credential
set it up, so an API token may expire without stopping it; disabling the
Mapping stops it.

## The Duty's Thread

Every Task of a Duty works in the Duty's own Thread. Its first Task opens it,
with the title and mission as its first message, and the Duty records it.
The Requester talks to the Duty there, and its Tasks report there. Deleting
the Thread stops the Duty.

## Other Duty Triggers

Besides its time trigger, a Duty wakes on the triggers its Requester adds with
`POST /api/duties/{id}/triggers`:

- `{"kind":"webhook"}` returns a URL under `/hooks/duties/{trigger}` and its
  signing secret, shown only in that response. A request is accepted only when
  `X-Hub-Signature-256` (as GitHub sends it) or `X-Aidash-Signature` carries
  `sha256=<hex HMAC-SHA256 of the body>`; an unknown trigger and a wrong
  signature are refused alike with 404. The body, at most 256 KiB of JSON or
  UTF-8 text, is the event. The secret is derived from the Deployment's API
  token, so replacing the token replaces every webhook secret.
- `{"kind":"duty_completed","source_duty_id":…}` wakes the Duty whenever a
  Task of that other Duty ends; the event names the Duty, the Task and its
  status.

An event is input, never a requester (ADR 0142). It goes to the Task that
booked it, if one did; otherwise it starts a new Task in the Duty's Thread
whose description is the mission followed by the event inside
`<event trust="data">`, marked as data that never directs the Agent. While
the Duty's previous Task is open, an event is skipped (`overlap`), the default
a Router may later replace. `GET /api/duties/{id}/events` lists what each
event did: `started`, `delivered`, `skipped` or `failed` with the reason.
Events are data of the Duty's Thread and go with the Duty.

A Run books the next event of a trigger of its root person's Duty by calling
`workspace_wait` with `on_trigger`. It sleeps until the event comes, for up to
30 days, and the event then wakes it: the wait's result carries the event as
`{"kind":"duty_event", …, "payload": …}`, data like every tool result. A Run
that arrived through Federation books none.

## Ticks

Every replica looks for Duties whose time trigger is due every
`[duties] poll_seconds` (15 by default). It claims due Duties with
`FOR UPDATE SKIP LOCKED` in the Requester's own transaction, starts the Task,
records the tick and moves the next time on, all in one transaction; a
scheduled time starts at most one Task.

| Tick                                                       | Recorded as                                                                            |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| The Task started                                           | `started`, then `completed`, `failed` or `cancelled` with its Task                     |
| The previous tick's Task is unfinished                     | `skipped (overlap)`, the default a Router may replace                                  |
| Aidash was down past the maximum lateness                  | `skipped (late)`; earlier missed ticks fold into the latest one, which runs once, late |
| The usage gate refused the Run                             | `skipped (limit: <code>)`                                                              |
| Any other refusal, such as `COMPACTION_DECIDER_UNAPPROVED` | `failed` with the reason; never retried silently                                       |
| A transient error, such as a database outage               | nothing; the next pass tries the tick again                                            |

After `pause_after_failures` failed Tasks in a row the Duty pauses itself. It
stops when its Requester leaves the Tenant or loses access, when its Agent
version can no longer be started, when its Thread is deleted and when the
Tenant is deleted (a restore leaves it stopped). Deleting the Workspace
deletes its Duties. The bell shows a **Duty stopped** notice to the Requester,
and to holders of `duty.administer` for a Duty waiting for a successor. A
Duty's Task completion appears in the bell without a toast; a failure also
toasts.

Only the Requester resumes a Duty. A holder of `duty.administer` may pause and
delete anyone's Duties and name a successor for one whose Requester left; it
runs again only when the successor resumes it, and they become its Requester.

## Agents

The core capability `duties` (`core.duties@1`) is on by default; an Agent
definition removes its tools with `remove_default`. `duty_create`,
`duty_update` and `duty_delete` (which also pauses) are Tool call approvals of
the person at the root of the Run's chain, decided by their Approval Mode and
Approval Rules; `duty_list` needs none. A Duty an Agent sets up acts as that
person and belongs to the Run's Workspace. A Run a Duty started may set up
Duties too (#1012): only the root person's authority and Budget bound what a
chain creates. A Run that arrived through Federation and a Run whose chain
starts at no person cannot set up or change Duties. A retried tool call
returns the Duty its first call set up.

A Duty an Agent sets up records the Task whose Run set it up, so the
[org chart](org-chart.md) shows the flow that made it, and stopping that flow
pauses it; only its Requester resumes it. A Duty may be staffed with a
generated agent only by the person whose request generated it, and the agent
stays live while such a Duty names it (see [generation](../generation.md)).
