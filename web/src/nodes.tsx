import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useSyncExternalStore,
} from "react";
import { useQuery } from "@tanstack/react-query";
import { discover, tenantNodes } from "./generated/aidash";
import { chooseNode, nodeStorageKey, type TenantNode } from "./node-selection";
import { ApiError } from "./transport";

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

/** The Nodes of `tenant`, which a Deployment Administrator may read for any
 *  Tenant it opens. Every reader of one Tenant shares one cache entry. */
export function useTenantNodes(tenant: string | null) {
  return useQuery({
    queryKey: ["nodes", tenant],
    queryFn: () => tenantNodes(encodeURIComponent(tenant!)),
    enabled: tenant !== null,
    refetchInterval: 30_000,
    // A refusal is the answer, and the dashboard waits for it.
    retry: (count, error) =>
      count < 1 && !(error instanceof ApiError && error.status < 500),
  });
}

/** Reads the Tenant's Nodes and keeps the selection among them. Call it once,
 *  where the Tenant is known, and share the result with `NodeSelectionContext`. */
export function useNodeSelection(tenant: string | null): NodeSelection {
  const list = useTenantNodes(tenant);
  const remembered = useSyncExternalStore(
    subscribe,
    () =>
      tenant === null ? null : localStorage.getItem(nodeStorageKey(tenant)),
    () => null,
  );
  const nodes = useMemo(
    () =>
      list.isError
        ? none
        : (list.data?.filter((node) => node.state === "live") ?? none),
    [list.isError, list.data],
  );
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

/** The query that reads discovery for the sending Node, so every view of it
 *  shares one cache entry per Node. Its local agents carry that Node's address. */
export function discoveryQuery(node: TenantNode | undefined) {
  return {
    queryKey: ["discovery", node?.name ?? null] as const,
    queryFn: () => discover({}, node ? { node: node.name } : undefined),
  };
}
