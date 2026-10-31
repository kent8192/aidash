"""Keep one Issue open while the development branch's Release CI is red (#606).

A failed run opens the Issue, or comments on the one already open; a green run
comments and closes it. The Issue is found by a body marker, not a label.
"""
import json
import os
import urllib.request

MARKER = "<!-- release-ci:develop-red -->"
LABELS = ["bug", "severity:high", "priority:P1"]
FOOTER = "Posted by Release CI (`release-ci.yml`)."


def plan(result, issue, branch, sha, run_url, failed_jobs):
    """Returns the GitHub writes for one completed run, as (method, path, body)."""
    commit = f"`{sha[:10]}`"
    if result == "success":
        if issue is None:
            return []
        number = issue["number"]
        return [
            ("POST", f"issues/{number}/comments", {"body": f"`{branch}` is green again at {commit}: {run_url}\n\n{FOOTER}"}),
            ("PATCH", f"issues/{number}", {"state": "closed", "state_reason": "completed"}),
        ]
    jobs = "\n".join(f"- {name}" for name in failed_jobs) or "- (no failed job reported; see the run)"
    report = f"Commit: {commit}\nRun: {run_url}\nResult: `{result}`\n\nFailed jobs:\n{jobs}"
    if issue is not None:
        return [("POST", f"issues/{issue['number']}/comments", {"body": f"`{branch}` is still red.\n\n{report}\n\n{FOOTER}"})]
    body = (
        f"{MARKER}\n## Summary\n\nRelease CI failed on `{branch}`, so open PRs may fail the same jobs.\n\n"
        f"{report}\n\nThis Issue closes itself when Release CI next passes on `{branch}`. "
        f"A flake also opens it; rerun the failed jobs or wait for the next merge.\n\n{FOOTER}"
    )
    return [("POST", "issues", {"title": f"{branch} Release CI is red", "body": body, "labels": LABELS})]


def github(method, path, body=None):
    request = urllib.request.Request(
        f"{os.environ['GITHUB_API_URL']}/repos/{os.environ['GITHUB_REPOSITORY']}/{path}",
        data=None if body is None else json.dumps(body).encode(),
        method=method,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {os.environ['GH_TOKEN']}",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def paged(path):
    page = 1
    while True:
        items = github("GET", f"{path}&per_page=100&page={page}")
        items = items.get("jobs", items) if isinstance(items, dict) else items
        yield from items
        if len(items) < 100:
            return
        page += 1


def open_issue():
    for issue in paged("issues?state=open"):
        if "pull_request" not in issue and MARKER in (issue.get("body") or ""):
            return issue
    return None


def main():
    run_id, attempt = os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"]
    result = os.environ["RESULT"]
    failed = []
    if result != "success":
        failed = [job["name"] for job in paged(f"actions/runs/{run_id}/attempts/{attempt}/jobs?filter=latest")
                  if job.get("conclusion") in ("failure", "timed_out")]
    run_url = f"{os.environ['GITHUB_SERVER_URL']}/{os.environ['GITHUB_REPOSITORY']}/actions/runs/{run_id}"
    for method, path, body in plan(result, open_issue(), os.environ["GITHUB_REF_NAME"], os.environ["GITHUB_SHA"], run_url, failed):
        github(method, path, body)
        print(method, path)


if __name__ == "__main__":
    main()
