import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useSyncExternalStore,
} from "react";
import { useQuery } from "@tanstack/react-query";
import { tenantNodes } from "./generated/aidash";
import type { Binding, Discovery, RequestSearch } from "./generated/models";
import { chooseNode, nodeStorageKey, type TenantNode } from "./node-selection";
import { apiFetch } from "./transport";

/** The Nodes of the signed-in Tenant and the one this browser works in. */
export type NodeSelection = {
  /** The Tenant the Nodes belong to; null for a Deployment Administrator. */
  tenant: string | null;
  nodes: readonly TenantNode[];
  /** Undefined while the list loads, and for a Deployment Administrator. */
  selected: TenantNode | undefined;
  /** The Tenant's default Node, where Catalog Approvals are shown and decided. */
  home: TenantNode | undefined;
  /** Remembers the Node in this browser, for this Tenant. */
  select: (name: string) => void;
  /** Whether the list is still being read for the first time. */
  pending: boolean;
};

const none: readonly TenantNode[] = [];
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  window.addEventListener("storage", listener);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", listener);
  };
}

/** Reads the Tenant's Nodes and keeps the selection among them. Call it once,
 *  where the Tenant is known, and share the result with `NodeSelectionContext`. */
export function useNodeSelection(tenant: string | null): NodeSelection {
  const list = useQuery({
    queryKey: ["nodes", tenant],
    queryFn: () => tenantNodes(encodeURIComponent(tenant!)),
    enabled: tenant !== null,
    refetchInterval: 30_000,
  });
  const remembered = useSyncExternalStore(
    subscribe,
    () =>
      tenant === null ? null : localStorage.getItem(nodeStorageKey(tenant)),
    () => null,
  );
  const nodes = list.isError ? none : (list.data ?? none);
  const select = useCallback(
    (name: string) => {
      if (tenant === null) return;
      localStorage.setItem(nodeStorageKey(tenant), name);
      for (const listener of listeners) listener();
    },
    [tenant],
  );
  return useMemo(
    () => ({
      tenant,
      nodes,
      selected: chooseNode(nodes, remembered),
      home: nodes.find((node) => node.default),
      select,
      pending: tenant !== null && list.isPending,
    }),
    [tenant, nodes, remembered, select, list.isPending],
  );
}

export const NodeSelectionContext = createContext<NodeSelection>({
  tenant: null,
  nodes: none,
  selected: undefined,
  home: undefined,
  select: () => undefined,
  pending: false,
});

export function useNodes(): NodeSelection {
  return useContext(NodeSelectionContext);
}

/**
 * `POST /api/discover`, sent from `node`: a Tenant's Node discovers what it
 * reaches, so the Node the person works in is the sending one. The contract
 * does not describe this query parameter, which is why the generated client
 * cannot send it.
 */
export function discoverFrom(
  node: string | undefined,
  search: RequestSearch = {},
): Promise<Discovery> {
  return apiFetch<Discovery>(
    `/api/discover${node ? `?node=${encodeURIComponent(node)}` : ""}`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(search),
    },
  );
}

/** The query that reads discovery for the sending Node, so every view of it
 *  shares one cache entry per Node. */
export function discoveryQuery(node: string | undefined) {
  return {
    queryKey: ["discovery", node ?? null] as const,
    queryFn: () => discoverFrom(node),
  };
}

/** `GET /api/authorization/{tenant}/catalog`: the Catalog Approvals of one
 *  Node. The contract does not describe the required `node` parameter. */
export function catalogBindings(
  tenant: string,
  node: string,
): Promise<Binding[]> {
  return apiFetch<Binding[]>(
    `/api/authorization/${encodeURIComponent(tenant)}/catalog?node=${encodeURIComponent(node)}`,
  );
}
