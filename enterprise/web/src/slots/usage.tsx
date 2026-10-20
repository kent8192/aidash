import { useQuery } from "@tanstack/react-query";
import { Download } from "lucide-react";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Alert } from "@/components/patterns";
import { authenticatedFetch, apiFetch } from "@/transport";
import { Empty, Panel, useI18n } from "@/ui";

type UsageRow = {
  tenant: string;
  date: string;
  pod_seconds: number;
  storage_bytes: number | null;
};
type UsageReport = { from: string; to: string; rows: UsageRow[] };

const copy = {
  "ja-JP": {
    title: "使用量",
    tenant: "Tenant",
    date: "日付 (UTC)",
    pod: "Execution Pod 時間 (秒)",
    storage: "保存バイト数",
    csv: "CSV をダウンロード",
    unavailable: "使用量を読み込めません。",
  },
  "en-US": {
    title: "Usage",
    tenant: "Tenant",
    date: "Date (UTC)",
    pod: "Execution Pod seconds",
    storage: "Stored bytes",
    csv: "Download CSV",
    unavailable: "Usage could not be loaded.",
  },
};

/** The report of this month so far, as the CSV export serves it. */
export function UsagePanel() {
  const { locale } = useI18n();
  const text = copy[locale];
  const report = useQuery({
    queryKey: ["usage"],
    queryFn: () => apiFetch<UsageReport>("/api/usage"),
  });
  const download = async () => {
    const response = await authenticatedFetch("/api/usage.csv");
    const url = URL.createObjectURL(await response.blob());
    const link = document.createElement("a");
    link.href = url;
    link.download = "usage.csv";
    link.click();
    URL.revokeObjectURL(url);
  };
  return (
    <Panel
      title={text.title}
      action={
        <button
          type="button"
          onClick={() => void download()}
          className="flex h-8 items-center gap-2 rounded-md px-2 text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground"
        >
          <Download aria-hidden className="size-3.5" />
          {text.csv}
        </button>
      }
    >
      {report.isError ? (
        <Alert retry={() => void report.refetch()}>{text.unavailable}</Alert>
      ) : !report.data?.rows.length ? (
        <Empty />
      ) : (
        <div className="min-w-0 rounded-md border border-border bg-surface">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{text.tenant}</TableHead>
                <TableHead>{text.date}</TableHead>
                <TableHead className="text-right">{text.pod}</TableHead>
                <TableHead className="text-right">{text.storage}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {report.data.rows.map((row) => (
                <TableRow key={`${row.tenant}:${row.date}`}>
                  <TableCell className="font-mono text-xs">
                    {row.tenant}
                  </TableCell>
                  <TableCell className="font-mono text-xs">
                    {row.date}
                  </TableCell>
                  <TableCell className="text-right font-mono">
                    {row.pod_seconds}
                  </TableCell>
                  <TableCell className="text-right font-mono">
                    {row.storage_bytes ?? ""}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </Panel>
  );
}
