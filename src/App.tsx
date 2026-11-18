import { PRODUCT_NAME, SUPPORTED_TARGETS } from "./meta";

export default function App() {
  return (
    <main className="app-shell">
      <header className="brand" aria-label={PRODUCT_NAME}>
        <span className="brand-mark" aria-hidden="true">
          &gt;_
        </span>
        <span>{PRODUCT_NAME}</span>
      </header>

      <section className="status-card" aria-labelledby="foundation-title">
        <p className="eyebrow">PROJECT FOUNDATION</p>
        <h1 id="foundation-title">一个入口，管理你的 AI 终端。</h1>
        <p className="description">
          菜单栏与托盘入口已就绪。关闭窗口只会收起
          AgentTerm；需要完全退出时，请使用托盘菜单中的「退出
          AgentTerm」。下一步将接入 Claude Code、Codex 和 Agy。
        </p>
        <div className="target-list" aria-label="支持的平台">
          {SUPPORTED_TARGETS.map((target) => (
            <span className="target" key={target.id}>
              {target.label}
            </span>
          ))}
        </div>
      </section>

      <footer className="footer">Tauri 2 · Rust · React</footer>
    </main>
  );
}
