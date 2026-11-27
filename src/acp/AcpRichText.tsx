import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";

type Segment =
  | { type: "text"; content: string }
  | { type: "code"; lang: string; content: string };

function splitCodeFences(source: string): Segment[] {
  const segments: Segment[] = [];
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  let mode: "text" | "code" = "text";
  let fenceLang = "";
  let buffer: string[] = [];

  const flush = () => {
    if (mode === "code") {
      segments.push({
        type: "code",
        lang: fenceLang,
        content: buffer.join("\n"),
      });
    } else if (buffer.length > 0) {
      const joined = buffer.join("\n");
      if (joined.length > 0) {
        segments.push({ type: "text", content: joined });
      }
    }
    buffer = [];
  };

  for (const line of lines) {
    if (mode === "text") {
      const open = line.match(/^```([A-Za-z0-9_+-]*)\s*$/);
      if (open) {
        flush();
        mode = "code";
        fenceLang = open[1] ?? "";
        continue;
      }
      buffer.push(line);
    } else {
      if (/^```\s*$/.test(line)) {
        flush();
        mode = "text";
        fenceLang = "";
        continue;
      }
      buffer.push(line);
    }
  }
  flush();
  return segments;
}

function renderInline(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const tokenRegex = /(`[^`\n]+`|\*\*[^*\n]+\*\*)/g;
  let lastIndex = 0;
  let key = 0;
  for (const match of text.matchAll(tokenRegex)) {
    const start = match.index ?? 0;
    if (start > lastIndex) {
      nodes.push(text.slice(lastIndex, start));
    }
    const token = match[0];
    if (token.startsWith("`") && token.endsWith("`")) {
      nodes.push(
        <code key={`ic-${key++}`} className="acp-inline-code">
          {token.slice(1, -1)}
        </code>,
      );
    } else if (token.startsWith("**") && token.endsWith("**")) {
      nodes.push(<strong key={`b-${key++}`}>{token.slice(2, -2)}</strong>);
    }
    lastIndex = start + token.length;
  }
  if (lastIndex < text.length) {
    nodes.push(text.slice(lastIndex));
  }
  return nodes;
}

function renderTextBlocks(text: string, blockKeyPrefix: string): ReactNode[] {
  const lines = text.split("\n");
  const blocks: ReactNode[] = [];
  let listType: "ul" | "ol" | null = null;
  let listItems: string[] = [];
  let paraLines: string[] = [];
  let index = 0;

  const flushParagraph = () => {
    if (paraLines.length === 0) return;
    blocks.push(
      <p key={`${blockKeyPrefix}-p-${index++}`} className="acp-rich-p">
        {paraLines.map((line, lineIdx) => (
          <Fragment key={lineIdx}>
            {lineIdx > 0 ? <br /> : null}
            {renderInline(line)}
          </Fragment>
        ))}
      </p>,
    );
    paraLines = [];
  };

  const flushList = () => {
    if (!listType || listItems.length === 0) return;
    const Tag = listType;
    blocks.push(
      <Tag key={`${blockKeyPrefix}-l-${index++}`} className="acp-rich-list">
        {listItems.map((item, itemIdx) => (
          <li key={itemIdx}>{renderInline(item)}</li>
        ))}
      </Tag>,
    );
    listType = null;
    listItems = [];
  };

  for (const line of lines) {
    const heading = line.match(/^(#{1,4})\s+(.+)$/);
    if (heading) {
      flushParagraph();
      flushList();
      blocks.push(
        <div
          key={`${blockKeyPrefix}-h-${index++}`}
          className={`acp-rich-heading is-h${heading[1].length}`}
        >
          {renderInline(heading[2])}
        </div>,
      );
      continue;
    }

    const bullet = line.match(/^\s*[-*]\s+(.+)$/);
    if (bullet) {
      flushParagraph();
      if (listType === "ol") flushList();
      listType = "ul";
      listItems.push(bullet[1]);
      continue;
    }

    const ordered = line.match(/^\s*\d+\.\s+(.+)$/);
    if (ordered) {
      flushParagraph();
      if (listType === "ul") flushList();
      listType = "ol";
      listItems.push(ordered[1]);
      continue;
    }

    const quote = line.match(/^>\s?(.*)$/);
    if (quote) {
      flushParagraph();
      flushList();
      blocks.push(
        <blockquote
          key={`${blockKeyPrefix}-q-${index++}`}
          className="acp-rich-quote"
        >
          {renderInline(quote[1])}
        </blockquote>,
      );
      continue;
    }

    if (line.trim() === "") {
      flushParagraph();
      flushList();
      continue;
    }

    flushList();
    paraLines.push(line);
  }

  flushParagraph();
  flushList();
  return blocks;
}

export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
    await writeText(text);
    return true;
  } catch {
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      try {
        await navigator.clipboard.writeText(text);
        return true;
      } catch {
        return false;
      }
    }
    return false;
  }
}

export function AcpCopyButton({
  text,
  label = "复制",
}: {
  text: string;
  label?: string;
}) {
  const [copied, setCopied] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => {
    return () => clearTimeout(timerRef.current);
  }, []);

  return (
    <button
      type="button"
      className="acp-copy-btn"
      onClick={() => {
        void copyToClipboard(text).then((ok) => {
          if (!ok) return;
          setCopied(true);
          clearTimeout(timerRef.current);
          timerRef.current = setTimeout(() => setCopied(false), 1600);
        });
      }}
    >
      {copied ? "已复制" : label}
    </button>
  );
}

export default function AcpRichText({ text }: { text: string }) {
  const segments = splitCodeFences(text);
  return (
    <div className="acp-rich">
      {segments.map((segment, index) =>
        segment.type === "code" ? (
          <div key={`seg-${index}`} className="acp-code-block">
            <div className="acp-code-header">
              <span>{segment.lang || "code"}</span>
              <AcpCopyButton text={segment.content} />
            </div>
            <pre className="acp-code-pre">
              <code>{segment.content}</code>
            </pre>
          </div>
        ) : (
          <Fragment key={`seg-${index}`}>
            {renderTextBlocks(segment.content, `seg-${index}`)}
          </Fragment>
        ),
      )}
    </div>
  );
}
