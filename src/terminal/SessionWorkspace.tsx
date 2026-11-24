import { useEffect, useRef, useState, type PointerEvent } from "react";
import SessionPanel from "./SessionPanel";
import type { ActiveSession, RenameResult } from "./useLauncher";

type Props = {
  sessions: ActiveSession[];
  paneIds: string[];
  activeId: string | null;
  focusRequest: number;
  closingId: string | null;
  onFocus: (id: string) => void;
  onClose: (id: string) => void;
  onExit: (id: string, reason: string) => void;
  onRename: (id: string, title: string) => Promise<RenameResult>;
  onPromptLine: (id: string, line: string) => void;
};

export function clampSplitRatio(value: number): number {
  return Number.isFinite(value) ? Math.min(70, Math.max(30, value)) : 50;
}

/** Every session keeps the same parent/key in either layout, including hidden tabs. */
export default function SessionWorkspace({
  sessions,
  paneIds,
  activeId,
  focusRequest,
  closingId,
  onFocus,
  onClose,
  onExit,
  onRename,
  onPromptLine,
}: Props) {
  const host = useRef<HTMLDivElement | null>(null);
  const [ratio, setRatio] = useState(50);
  const [stacked, setStacked] = useState(false);
  const split = paneIds.length === 2;
  const showing = activeId !== null;

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => {
      // Hidden launch view must not overwrite the last usable orientation.
      if (entry.contentRect.width > 0)
        setStacked(entry.contentRect.width < 720);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const drag = (event: PointerEvent<HTMLDivElement>) => {
    if (
      !event.currentTarget.hasPointerCapture(event.pointerId) ||
      !host.current
    )
      return;
    const rect = host.current.getBoundingClientRect();
    const length = (stacked ? rect.height : rect.width) - 8;
    if (length <= 0) return;
    const offset = stacked
      ? event.clientY - rect.top
      : event.clientX - rect.left;
    setRatio(clampSplitRatio(((offset - 4) / length) * 100));
  };

  return (
    <div
      ref={host}
      className={`workspace-panes${split ? " is-split" : ""}${stacked ? " is-stacked" : ""}`}
      hidden={!showing}
      aria-label={split ? "双终端工作区" : "单终端工作区"}
      style={{
        gridTemplateColumns:
          split && !stacked
            ? `minmax(0, ${ratio}fr) 8px minmax(0, ${100 - ratio}fr)`
            : "minmax(0, 1fr)",
        gridTemplateRows:
          split && stacked
            ? `minmax(0, ${ratio}fr) 8px minmax(0, ${100 - ratio}fr)`
            : "minmax(0, 1fr)",
      }}
    >
      {sessions.map((session) => {
        const position = paneIds.indexOf(session.info.id);
        const visible = showing && position >= 0;
        const focused = visible && activeId === session.info.id;
        return (
          <div
            key={session.info.id}
            className="workspace-terminal-stage"
            id={`pane-${session.info.id}`}
            role="tabpanel"
            aria-labelledby={`tab-${session.info.id}`}
            hidden={!visible}
            data-focused={focused}
            style={{
              gridColumn: split && !stacked && position === 1 ? 3 : 1,
              gridRow: split && stacked && position === 1 ? 3 : 1,
            }}
            onPointerDownCapture={() => onFocus(session.info.id)}
            onFocusCapture={() => onFocus(session.info.id)}
          >
            <SessionPanel
              session={session.info}
              label={session.label}
              visible={visible}
              focused={focused}
              focusRequest={focusRequest}
              running={session.running}
              exitReason={session.exitReason}
              busy={closingId === session.info.id}
              closeError={session.closeError}
              onClose={() => onClose(session.info.id)}
              onExit={onExit}
              onRename={(title) => onRename(session.info.id, title)}
              onPromptLine={onPromptLine}
            />
          </div>
        );
      })}
      {split ? (
        <div
          className="workspace-splitter"
          role="separator"
          aria-label="调整终端分屏"
          aria-orientation={stacked ? "horizontal" : "vertical"}
          aria-controls={`pane-${paneIds[0]}`}
          aria-valuemin={30}
          aria-valuemax={70}
          aria-valuenow={Math.round(ratio)}
          aria-valuetext={`首个终端占 ${Math.round(ratio)}%`}
          title="拖动调整 · 方向键微调 · 双击等分"
          tabIndex={0}
          style={{ gridColumn: stacked ? 1 : 2, gridRow: stacked ? 2 : 1 }}
          onPointerDown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            event.currentTarget.focus();
            event.currentTarget.setPointerCapture(event.pointerId);
          }}
          onPointerMove={drag}
          onPointerUp={(event) => {
            if (event.currentTarget.hasPointerCapture(event.pointerId))
              event.currentTarget.releasePointerCapture(event.pointerId);
          }}
          onPointerCancel={(event) => {
            if (event.currentTarget.hasPointerCapture(event.pointerId))
              event.currentTarget.releasePointerCapture(event.pointerId);
          }}
          onDoubleClick={() => setRatio(50)}
          onKeyDown={(event) => {
            const previous = stacked ? "ArrowUp" : "ArrowLeft";
            const next = stacked ? "ArrowDown" : "ArrowRight";
            if (![previous, next, "Home", "End", "Enter"].includes(event.key))
              return;
            event.preventDefault();
            setRatio((value) =>
              clampSplitRatio(
                event.key === "Home"
                  ? 30
                  : event.key === "End"
                    ? 70
                    : event.key === "Enter"
                      ? 50
                      : value + (event.key === next ? 2 : -2),
              ),
            );
          }}
        />
      ) : null}
    </div>
  );
}
