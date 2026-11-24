import { Component, type ErrorInfo, type ReactNode } from "react";
import { LAYOUT_STORAGE_KEY } from "./layout";

type Props = { children: ReactNode };
type State = { error: Error | null };

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("AgentTerm renderer failed", error, info);
  }

  resetLayout = () => {
    try {
      localStorage.removeItem(LAYOUT_STORAGE_KEY);
    } catch {
      // Reloading still gives the renderer another chance to start.
    }
    window.location.reload();
  };

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <main className="renderer-error" role="alert">
        <p className="workspace-kicker">AGENTTERM RECOVERY</p>
        <h1>工作区暂时无法显示</h1>
        <p>
          已保护当前会话进程。可以重置本地布局并重新加载界面，已有 CLI
          会话不会因此被结束。
        </p>
        <details>
          <summary>错误详情</summary>
          <pre>{this.state.error.message}</pre>
        </details>
        <button
          type="button"
          className="primary-button"
          onClick={this.resetLayout}
        >
          重置布局并重新加载
        </button>
      </main>
    );
  }
}
