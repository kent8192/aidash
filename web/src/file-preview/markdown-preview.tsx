import { useEffect, useMemo, useState } from "react";
import { Notice } from "../components/patterns";
import { Button } from "../components/ui/button";
import { cn } from "../lib/utils";
import { useI18n } from "../ui";
import { imageMediaType, isPreviewImageType } from "./image.ts";
import { exceedsFilePreviewLimit, FILE_PREVIEW_BYTE_LIMIT } from "./limits.ts";
import {
  isHttpSafeUrl,
  parseMarkdown,
  type MarkdownBlock,
  type MarkdownInline,
} from "./markdown.ts";
import { FilePreviewLimitState } from "./states.tsx";
import { decodeUtf8 } from "./text.ts";
import type { FilePreviewVersion, FileVersionResolver } from "./types.ts";

export interface MarkdownFilePreviewProps extends FilePreviewVersion {
  resolveRelativePath?: FileVersionResolver;
  className?: string;
  onDownload?: (version: FilePreviewVersion) => void;
}

/**
 * Markdown File preview. The renderer never emits raw HTML: the parser drops
 * HTML tags, external links are plain text unless they are http/https, and
 * images are resolved as sibling File versions of the same revision through
 * the caller's resolver.
 */
export function MarkdownFilePreview({
  bytes,
  mediaType,
  fileName,
  resolveRelativePath,
  className,
  onDownload,
}: MarkdownFilePreviewProps) {
  const { t } = useI18n();
  const decoded = useMemo(() => decodeUtf8(bytes), [bytes]);
  const blocks = useMemo(
    () => (decoded.ok ? parseMarkdown(decoded.text) : []),
    [decoded],
  );

  if (exceedsFilePreviewLimit(bytes)) {
    return (
      <FilePreviewLimitState
        version={{ bytes, mediaType, fileName }}
        onDownload={onDownload}
      />
    );
  }
  if (!decoded.ok) {
    return (
      <div className="grid min-w-0 gap-2">
        <Notice tone="warning">{t("filePreviewMarkdownNotUtf8")}</Notice>
        {onDownload && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="w-fit"
            onClick={() => onDownload({ bytes, mediaType, fileName })}
          >
            {t("filePreviewDownload")}
          </Button>
        )}
      </div>
    );
  }
  return (
    <div
      className={cn(
        "grid min-w-0 gap-3 text-sm leading-relaxed [overflow-wrap:anywhere]",
        className,
      )}
    >
      {blocks.map((block, index) => (
        <MarkdownBlockView
          key={index}
          block={block}
          resolveRelativePath={resolveRelativePath}
        />
      ))}
    </div>
  );
}

function MarkdownBlockView({
  block,
  resolveRelativePath,
}: {
  block: MarkdownBlock;
  resolveRelativePath?: FileVersionResolver;
}) {
  switch (block.kind) {
    case "heading": {
      const Tag = (["h1", "h2", "h3", "h4", "h5", "h6"] as const)[
        block.level - 1
      ];
      return (
        <Tag className="mt-2 font-semibold text-foreground first:mt-0">
          <InlineNodes
            nodes={block.children}
            resolveRelativePath={resolveRelativePath}
          />
        </Tag>
      );
    }
    case "paragraph":
      return (
        <p>
          <InlineNodes
            nodes={block.children}
            resolveRelativePath={resolveRelativePath}
          />
        </p>
      );
    case "code":
      return (
        <pre className="max-h-[60vh] overflow-auto rounded-md border border-border bg-raised p-3 font-mono text-xs leading-relaxed text-foreground">
          <code>{block.text}</code>
        </pre>
      );
    case "list": {
      const items = block.items.map((item, index) => (
        <li key={index}>
          <InlineNodes nodes={item} resolveRelativePath={resolveRelativePath} />
        </li>
      ));
      return block.ordered ? (
        <ol className="grid list-decimal gap-1 pl-5">{items}</ol>
      ) : (
        <ul className="grid list-disc gap-1 pl-5">{items}</ul>
      );
    }
    case "blockquote":
      return (
        <blockquote className="border-l-2 border-border pl-3 text-muted-foreground">
          <div className="grid gap-2">
            {block.blocks.map((child, index) => (
              <MarkdownBlockView
                key={index}
                block={child}
                resolveRelativePath={resolveRelativePath}
              />
            ))}
          </div>
        </blockquote>
      );
    case "rule":
      return <hr className="border-border" />;
  }
}

function InlineNodes({
  nodes,
  resolveRelativePath,
}: {
  nodes: MarkdownInline[];
  resolveRelativePath?: FileVersionResolver;
}) {
  return (
    <>
      {nodes.map((node, index) => {
        switch (node.kind) {
          case "text":
            return <span key={index}>{node.text}</span>;
          case "strong":
            return <strong key={index}>{node.text}</strong>;
          case "em":
            return <em key={index}>{node.text}</em>;
          case "code":
            return (
              <code
                key={index}
                className="rounded bg-raised px-1 py-0.5 font-mono text-[0.9em]"
              >
                {node.text}
              </code>
            );
          case "link":
            return isHttpSafeUrl(node.href) ? (
              <a
                key={index}
                href={node.href}
                target="_blank"
                rel="noopener noreferrer"
                className="text-brand underline underline-offset-2"
              >
                {node.text}
              </a>
            ) : (
              <span key={index}>{node.text}</span>
            );
          case "image":
            return (
              <MarkdownImage
                key={`${node.path}:${index}`}
                path={node.path}
                alt={node.alt}
                resolveRelativePath={resolveRelativePath}
              />
            );
          case "image-refused":
            return (
              <ImageRefusal
                key={index}
                alt={node.alt}
                note="filePreviewMarkdownImageRefused"
              />
            );
        }
      })}
    </>
  );
}

function MarkdownImage({
  path,
  alt,
  resolveRelativePath,
}: {
  path: string;
  alt: string;
  resolveRelativePath?: FileVersionResolver;
}) {
  const [state, setState] = useState<
    | { status: "pending" }
    | { status: "ready"; url: string }
    | { status: "refused"; note: string }
  >({ status: "pending" });

  useEffect(() => {
    let cancelled = false;
    let objectUrl: string | undefined;
    void Promise.resolve()
      .then(() => resolveRelativePath?.(path) ?? null)
      .then((version) => {
        if (cancelled) return;
        if (!version) {
          setState({
            status: "refused",
            note: "filePreviewMarkdownImageMissing",
          });
          return;
        }
        if (version.bytes.byteLength > FILE_PREVIEW_BYTE_LIMIT) {
          setState({
            status: "refused",
            note: "filePreviewMarkdownImageTooLarge",
          });
          return;
        }
        if (!isPreviewImageType(version.mediaType, version.fileName || path)) {
          setState({
            status: "refused",
            note: "filePreviewMarkdownImageNotImage",
          });
          return;
        }
        objectUrl = URL.createObjectURL(
          new Blob([version.bytes], {
            type: imageMediaType(version.mediaType, version.fileName || path),
          }),
        );
        setState({ status: "ready", url: objectUrl });
      })
      .catch(() => {
        if (!cancelled) {
          setState({
            status: "refused",
            note: "filePreviewMarkdownImageMissing",
          });
        }
      });
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [path, resolveRelativePath]);

  if (state.status === "pending") {
    return <span className="text-muted-foreground">{alt || path}</span>;
  }
  if (state.status === "refused") {
    return <ImageRefusal alt={alt || path} note={state.note} />;
  }
  return (
    <img
      src={state.url}
      alt={alt}
      className="my-1 max-h-[60vh] w-auto max-w-full rounded-md border border-border bg-background object-contain"
    />
  );
}

function ImageRefusal({ alt, note }: { alt: string; note: string }) {
  const { t } = useI18n();
  return (
    <span className="text-muted-foreground">
      {alt} <span className="text-faint">({t(note)})</span>
    </span>
  );
}
