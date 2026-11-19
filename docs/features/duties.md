# Schedules

A **Schedule** starts a Task for one Agent of the Tenant on a recurring time,
with a fixed request (#557). It always acts as one person, its **Registrant**:
every Run it starts has the Registrant at the root of its delegation chain, so
their Approval Mode, Approval Rules, Plan Connections and MCP OAuth Connections
apply exactly as for a Run they start by hand (ADR 0134). There are no
Tenant-owned Schedules and no one-shot Schedules.

## Registering

A person registers a Schedule in a Workspace's **Schedules** tab, or through
`POST /api/workspaces/{workspace}/schedules`. It needs `workspace.read`,
`schedule.create` and `task.create` on the Workspace; members hold all three by
default.

```json
{
  "title": "Daily report",
  "request": "Summarize yesterday's merged pull requests.",
  "agent": { "id": "reporter", "version": "1.0.0" },
  "trigger": {
    "kind": "cron",
    "expression": "0 9 * * 1-5",
    "time_zone": "Asia/Tokyo"
  },
  "thread": { "kind": "new" },
  "approval_mode": "dont_ask",
  "max_lateness_seconds": 3600
}
```

- `trigger` is a five-field cron expression (minute, hour, day of month, month,
  day of week, where Sunday is 0 or 7) in an IANA time zone, or
  `{"kind":"interval","every_seconds":N}` counted from registration or resume.
  No two fire times may be closer than the minimum interval: the Deployment's
  (`[schedules] min_interval_seconds`, 15 minutes by default), which a Tenant
  may only raise. A small hidden jitter delays each tick by up to 30 seconds.
  A time a daylight-saving change skips runs once at the first instant after
  the gap (`30 2 * * *` in New York runs at 03:00 that day); a repeated time
  runs once.
- `thread` opens a new Thread on every tick (the default) or always uses
  `{"kind":"fixed","thread_id":…}` of the same Workspace.
- `approval_mode` is the mode of the Schedule's Runs, within what the Tenant's
  policy lets the Registrant use. Without it, their own default applies.
- The Agent version is pinned. The Schedule's page shows when a newer version
  exists; the Registrant moves it with an edit.

The Registrant edits, pauses, resumes, deletes or runs a Schedule now from the
Schedules tab or **Settings → Schedules** ("my Schedules"). "Run now" starts an
ordinary Run as the person who clicked it, in their own Approval Mode, and
records it in the history.

A Schedule runs on its Registrant's Mapping credential, whichever credential
registered it, so an API token may expire without stopping it; disabling the
Mapping stops it.

## Ticks

Every replica runs the scheduler every `[schedules] poll_seconds` (15 by
default). It claims due Schedules with `FOR UPDATE SKIP LOCKED` in the
Registrant's own transaction, starts the Run, records the tick and moves the
next time on, all in one transaction; a scheduled time starts at most one Task.

| Tick                                                                        | Recorded as                                                                            |
| --------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| The Run started                                                             | `started`, then `completed`, `failed` or `cancelled` with its Task                     |
| The previous tick's Run is unfinished                                       | `skipped (overlap)`                                                                    |
| Aidash was down past the maximum lateness                                   | `skipped (late)`; earlier missed ticks fold into the latest one, which runs once, late |
| The usage gate refused the Run                                              | `skipped (limit: <code>)`                                                              |
| Sooner than the minimum interval after the previous tick that started a Run | `skipped (interval)`                                                                   |
| Any other refusal, such as `COMPACTION_DECIDER_UNAPPROVED`                  | `failed` with the reason; never retried silently                                       |
| A transient error, such as a database outage                                | nothing; the next pass tries the tick again                                            |

After three failed Runs in a row the Schedule pauses itself. It stops when its
Registrant leaves the Tenant or loses access, when its Agent version can no
longer be started, when its fixed Thread is deleted, when the Tenant raises its
minimum interval above the Schedule's (the notice names the new minimum) and
when the Tenant is deleted (a restore leaves it stopped). Deleting the Workspace deletes its
Schedules. The bell shows a **Schedule stopped** notice to the Registrant, and
to holders of `schedule.administer` for a Schedule waiting for a successor. A
scheduled Run's completion appears in the bell without a toast; a failure also
toasts.

Only the Registrant resumes a Schedule. A holder of `schedule.administer` may
pause and delete anyone's Schedules and name a successor for one whose
Registrant left; it runs again only when the successor resumes it, and they
become its Registrant.

## Agents

The core capability `schedules` is on by default; an Agent definition removes
its tools with `remove_default`. `schedule_create`, `schedule_update` and
`schedule_delete` (which also pauses) are Tool call approvals of the person at
the root of the Run's chain, decided by their Approval Mode and Approval Rules;
`schedule_list` needs none. A Schedule an Agent registers acts as that person
and belongs to the Run's Workspace. A Run that arrived through Federation, a
Run whose chain starts at no person, and a Run a Schedule started (or any Run
it delegates to, whatever Task, or assigns through a generated Agent) cannot
register or change Schedules. A retried
tool call returns the Schedule its first call registered.

## Limits

`GET`/`PUT /api/schedules/limits` read and set the Tenant's limits: the total
number of Schedules (50 by default), the number per person (10) and the
minimum interval. Setting them needs `schedule.administer`. They stay within the
Deployment's `[schedules]` ceilings, and on Cloud the covering Tier's
`schedules` cap may lower the total. The response also gives the limits that
apply now.
