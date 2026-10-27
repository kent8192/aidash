import { execFileSync } from "node:child_process";
import { expect, type APIRequestContext } from "@playwright/test";

/** Creates a Tenant through the creation API, which assigns its name (ADR
 *  0050), and returns that name. Its initial policy is revision 1. */
export async function createTenant(
  request: APIRequestContext,
  token: string,
  label: string,
): Promise<string> {
  const response = await request.post("/api/tenants", {
    headers: { authorization: `Bearer ${token}` },
    data: { label },
  });
  expect(response.status(), await response.text()).toBe(201);
  return ((await response.json()) as { name: string }).name;
}

/** A Subject Credential for `subject` of `tenant`, issued from the host with
 *  `manage issue-subject-credential` (#484). A Deployment Administrator holds
 *  no Tenant authority, so the first credential of a Tenant comes from whoever
 *  operates the host. golden_path.py names the command and the server's own
 *  environment in AIDASH_E2E_MANAGE and AIDASH_E2E_HOST_ENV (JSON). The
 *  Tenant's policy must already define and enable `subject`. */
export function issueSubjectCredential(
  tenant: string,
  subject: string,
  expiresInSeconds = 3600,
): { token: string; credential: { id: string; subject: string } } {
  const manage = process.env.AIDASH_E2E_MANAGE;
  const hostEnvironment = process.env.AIDASH_E2E_HOST_ENV;
  if (!manage || !hostEnvironment)
    throw new Error("AIDASH_E2E_MANAGE and AIDASH_E2E_HOST_ENV are required");
  const output = execFileSync(
    manage,
    [
      "issue-subject-credential",
      "--tenant",
      tenant,
      "--subject",
      subject,
      "--label",
      "e2e_host",
      "--expires-in-seconds",
      String(expiresInSeconds),
    ],
    {
      env: { ...process.env, ...JSON.parse(hostEnvironment) },
      encoding: "utf8",
      timeout: 120_000,
    },
  );
  return JSON.parse(output.trim().split("\n").pop()!);
}

/** The Node a new Tenant starts with. A Catalog Approval names the Node it
 *  applies to, and an approved agent's subject is keyed by the Node's address,
 *  `<address>/agents/<id>@<version>`. */
export async function defaultNode(
  request: APIRequestContext,
  token: string,
  tenant: string,
): Promise<{ name: string; address: string }> {
  const response = await request.get(`/api/tenants/${tenant}/nodes`, {
    headers: { authorization: `Bearer ${token}` },
  });
  expect(response.status(), await response.text()).toBe(200);
  const nodes = (await response.json()) as {
    name: string;
    address: string;
    default: boolean;
  }[];
  return nodes.find((node) => node.default)!;
}

/** The golden path's subject in Node A's research Tenant. Only Tenant
 *  subjects do Tenant work (#371), so live specs that create Workspaces or
 *  assign the golden path's agents act as it. golden_path.py issues the
 *  credential just before the browser run. */
export function acceptanceSubject(): {
  token: string;
  tenant: string;
  name: string;
} {
  const token = process.env.AIDASH_E2E_TOKEN;
  const tenant = process.env.AIDASH_E2E_TENANT;
  const name = process.env.AIDASH_E2E_SUBJECT;
  if (!token || !tenant || !name)
    throw new Error(
      "AIDASH_E2E_TOKEN, AIDASH_E2E_TENANT and AIDASH_E2E_SUBJECT are required",
    );
  return { token, tenant, name };
}
