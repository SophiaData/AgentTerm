import { useState } from "react";
import { displayPath } from "../projects/displayPath";
import TerminalView from "./TerminalView";
import { SESSION_TITLE_MAX_CHARS, normalizeManualTitle } from "./sessionNaming";
import { sessionTitle, type SessionInfo } from "./types";
import type { RenameResult } from "./useLauncher";

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
  onRename: (title: string) => Promise<RenameResult>;
  onPromptLine: (id: string, line: string) => void;
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
  onRename,
  onPromptLine,
}: Props) {
  const [confirmClose, setConfirmClose] = useState(false);
  const [editingTitle, setEditingTitle] = useState(false);
  const [draftTitle, setDraftTitle] = useState(label);
  const [savingTitle, setSavingTitle] = useState(false);
  const [renameError, setRenameError] = useState<string | null>(null);

  const beginRename = () => {
    setDraftTitle(label);
    setRenameError(null);
    setEditingTitle(true);
  };

  const cancelRename = () => {
    setEditingTitle(false);
    setRenameError(null);
  };

  const submitRename = async () => {
    if (savingTitle) return;
    const checked = normalizeManualTitle(draftTitle);
    if (!checked.ok) {
      setRenameError(checked.error);
      return;
    }
    if (checked.title === label) {
      setEditingTitle(false);
      setRenameError(null);
      return;
    }
    setSavingTitle(true);
    setRenameError(null);
    const result = await onRename(checked.title);
    setSavingTitle(false);
    if (result.ok) {
      setEditingTitle(false);
    } else {
      setRenameError(result.error);
    }
  };

  const readablePath = displayPath(session.projectPath);
  return (
    <section className="session-panel" aria-label="终端会话" hidden={!visible}>
      <div className="session-header">
        <div className="session-title-group">
          {editingTitle ? (
            <form
              className="session-rename-form"
              onSubmit={(event) => {
                event.preventDefault();
                void submitRename();
              }}
            >
              <input
                type="text"
                className="session-rename-input"
                aria-label="编辑会话名称"
                value={draftTitle}
                maxLength={SESSION_TITLE_MAX_CHARS}
                disabled={savingTitle}
                autoFocus
                onChange={(event) => setDraftTitle(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    cancelRename();
                  }
                }}
              />
              <button
                type="submit"
                className="ghost-button"
                disabled={savingTitle}
              >
                {savingTitle ? "保存中…" : "保存"}
              </button>
              <button
                type="button"
                className="ghost-button"
                disabled={savingTitle}
                onClick={cancelRename}
              >
                取消
              </button>
            </form>
          ) : (
            <>
              <span
                className="session-title"
                title={`${label}（${sessionTitle(session)}）· 双击改名`}
                onDoubleClick={beginRename}
              >
                {label}
              </span>
              <button
                type="button"
                className="ghost-button session-rename-trigger"
                aria-label="重命名会话"
                title="修改会话名称"
                disabled={busy}
                onClick={beginRename}
              >
                改名
              </button>
            </>
          )}
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
      {renameError ? (
        <p className="session-close-error" role="alert">
          {renameError}
        </p>
      ) : null}
      <p className="session-context" title={readablePath}>
        {readablePath}
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
        onPromptLine={onPromptLine}
      />
    </section>
  );
}
