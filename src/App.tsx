import { PRODUCT_NAME, SUPPORTED_TARGETS } from "./meta";
import ToolList from "./tools/ToolList";
import { useTools } from "./tools/useTools";

export default function App() {
  const { tools, loading, error, refresh } = useTools();

  return (
    <main className="app-shell">
      <header className="brand" aria-label={PRODUCT_NAME}>
        <span className="brand-mark" aria-hidden="true">
          &gt;_
        </span>
        <span>{PRODUCT_NAME}</span>
      </header>

      <section className="status-card" aria-labelledby="foundation-title">
        <p className="eyebrow">MILESTONE 1</p>
        <h1 id="foundation-title">一个入口，管理你的 AI 终端。</h1>
        <p className="description">
          关闭窗口只会收起 AgentTerm；需要完全退出时，请使用托盘菜单中的「退出
          AgentTerm」。
        </p>
        <div className="target-list" aria-label="支持的平台">
          {SUPPORTED_TARGETS.map((target) => (
            <span className="target" key={target.id}>
              {target.label}
            </span>
          ))}
        </div>
      </section>

      <ToolList
        tools={tools}
        loading={loading}
        error={error}
        onRefresh={refresh}
      />

      <footer className="footer">Tauri 2 · Rust · React</footer>
    </main>
  );
}
