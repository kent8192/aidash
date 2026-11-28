import { apiFetch } from "../transport";
import { remotePreviewUrl } from "./remote-preview-url";

export type RemotePreviewTarget = { kind: "live"; live_preview_id: string };

export type RemotePreviewInspection = {
  live_preview: { id: string; state: string } | null;
  preview_available: boolean;
};

type RemotePreviewTicket = {
  url: string;
  expires_at: string;
  preview_domain: string;
};

export const inspectRemotePreview = (run: string, signal?: AbortSignal) =>
  apiFetch<RemotePreviewInspection>(`/api/executions/${run}/remote-preview`, {
    signal,
  });

export async function issueRemotePreview(
  run: string,
  target: RemotePreviewTarget,
): Promise<string> {
  const ticket = await apiFetch<RemotePreviewTicket>(
    `/api/executions/${run}/preview-tickets`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(target),
    },
  );
  return remotePreviewUrl(ticket);
}
