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
