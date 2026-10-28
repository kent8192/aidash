import { apiFetch } from "../transport";
import { remotePreviewUrl } from "./remote-preview-url";

export type RemotePreviewTarget =
  | { kind: "file"; area_id: string; file_id: string }
  | { kind: "live"; live_preview_id: string };

export type RemotePreviewInspection = {
  area_id: string;
  files: {
    file_id: string;
    path: string;
    digest: string;
    size: number;
    media_type: string;
  }[];
  live_preview: { id: string; state: string } | null;
  preview_available: boolean;
};

type RemotePreviewTicket = {
  url: string;
  expires_at: string;
  preview_domain: string;
};

export const inspectRemotePreview = (run: string, signal?: AbortSignal) =>
  apiFetch<RemotePreviewInspection>(`/api/runs/${run}/remote-preview`, {
    signal,
  });

export async function issueRemotePreview(
  run: string,
  target: RemotePreviewTarget,
): Promise<string> {
  const ticket = await apiFetch<RemotePreviewTicket>(
    `/api/runs/${run}/preview-tickets`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(target),
    },
  );
  return remotePreviewUrl(ticket);
}
