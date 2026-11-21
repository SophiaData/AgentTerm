import { useState } from "react";
import TerminalView from "./TerminalView";
import { sessionTitle, type SessionInfo } from "./types";

type Props = {
  session: SessionInfo;
  label: string;
  visible: boolean;
  focused: boolean;
  focusRequest: number;
  running: boolean;
  exitReason: string | null;
  busy: boolean;
  closeError?: string | null;
  onClose: () => void;
  onExit: (id: string, reason: string) => void;
};

export default function SessionPanel({
  session,
  label,
  visible,
  focused,
  focusRequest,
  running,
  exitReason,
  busy,
  closeError,
  onClose,
  onExit,
}: Props) {
  const [confirmClose, setConfirmClose] = useState(false);
  return (
    <section className="session-panel" aria-label="终端会话" hidden={!visible}>
      <div className="session-header">
        <div className="session-title-group">
          <span className="session-title" title={sessionTitle(session)}>
            {label}
          </span>
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
          onClick={() => (running ? setConfirmClose(true) : onClose())}
          disabled={busy}
        >
          {busy ? "正在结束…" : running ? "结束会话" : "关闭会话"}
        </button>
      </div>
      <p className="session-context" title={session.projectPath}>
        {session.projectPath}
      </p>
      {confirmClose && running ? (
        <div className="session-confirm" role="group" aria-label="确认结束会话">
          <span>结束会停止这个 CLI。尚未提交的输入可能丢失。</span>
          <button
            type="button"
            className="ghost-button"
            autoFocus
            disabled={busy}
            onClick={() => setConfirmClose(false)}
          >
            继续使用
          </button>
          <button
            type="button"
            className="danger-button"
            disabled={busy}
            onClick={onClose}
          >
            确认结束
          </button>
        </div>
      ) : null}
      {closeError ? (
        <p className="session-close-error" role="alert">
          {closeError}
        </p>
      ) : null}
      <TerminalView
        session={session}
        visible={visible}
        focused={focused}
        focusRequest={focusRequest}
        running={running}
        busy={busy}
        onExit={onExit}
      />
    </section>
  );
}
