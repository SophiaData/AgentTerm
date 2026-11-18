import TerminalView from "./TerminalView";
import { sessionTitle, type SessionInfo } from "./types";

type Props = {
  session: SessionInfo;
  running: boolean;
  exitReason: string | null;
  busy: boolean;
  closeError?: string | null;
  onClose: () => void;
  onExit: (id: string, reason: string) => void;
};

export default function SessionPanel({
  session,
  running,
  exitReason,
  busy,
  closeError,
  onClose,
  onExit,
}: Props) {
  return (
    <section className="session-panel" aria-label="终端会话">
      <div className="session-header">
        <div className="session-title-group">
          <span className="session-title">{sessionTitle(session)}</span>
          <span className={`badge ${running ? "badge-installed" : ""}`}>
            {running
              ? "运行中"
              : exitReason
                ? `已退出：${exitReason}`
                : "已结束"}
          </span>
        </div>
        <button
          type="button"
          className="ghost-button"
          onClick={onClose}
          disabled={busy}
        >
          关闭会话
        </button>
      </div>
      {closeError ? (
        <p className="session-close-error" role="alert">
          {closeError}
        </p>
      ) : null}
      <TerminalView session={session} onExit={onExit} />
    </section>
  );
}
