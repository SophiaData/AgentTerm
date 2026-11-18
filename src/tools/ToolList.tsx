import { badgeLabel, detailText, type ToolStatus } from "./types";

type Props = {
  tools: ToolStatus[];
  loading: boolean;
  error: string | null;
  onRefresh: () => void;
};

export default function ToolList({ tools, loading, error, onRefresh }: Props) {
  const showInitialLoading = loading && tools.length === 0;

  return (
    <section className="tools" aria-labelledby="tools-title">
      <div className="tools-header">
        <h2 id="tools-title">AI 工具</h2>
        <button
          type="button"
          className="refresh-button"
          onClick={onRefresh}
          disabled={loading}
        >
          {loading ? "检测中…" : "重新检测"}
        </button>
      </div>

      {error ? (
        <p className="tools-error" role="alert">
          {error}
        </p>
      ) : null}

      {showInitialLoading ? (
        <p className="tools-empty">正在检测 Claude Code、Codex 和 Agy…</p>
      ) : (
        <ul className="tool-grid">
          {tools.map((tool) => (
            <li className="tool-card" key={tool.id}>
              <div className="tool-card-top">
                <span className="tool-name">{tool.name}</span>
                <span className={`badge badge-${tool.state.kind}`}>
                  {badgeLabel(tool.state)}
                </span>
              </div>
              <p className="tool-detail">{detailText(tool.state)}</p>
              {tool.state.kind === "installed" ? (
                <p className="tool-path" title={tool.state.path}>
                  {tool.state.path}
                </p>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
