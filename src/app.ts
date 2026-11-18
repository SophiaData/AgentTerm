import type { Backend } from "./api";
import { announce, button, element } from "./dom";
import {
  statusDetail,
  statusText,
  type Project,
  type ToolStatus,
} from "./types";

type AppState = {
  projects: Project[];
  tools: ToolStatus[];
  selectedProjectId: string | null;
  selectedToolId: string | null;
  pendingRemovalId: string | null;
  loadingProjects: boolean;
  loadingTools: boolean;
  busy: boolean;
  projectError: string | null;
  toolsError: string | null;
  launchError: string | null;
  notice: string | null;
};

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Native-DOM app: a small state model and one delegated click listener. */
export function createApp(root: HTMLElement, api: Backend) {
  let destroyed = false;
  let desiredFocus: { action: string; id?: string } | null = null;
  const state: AppState = {
    projects: [],
    tools: [],
    selectedProjectId: null,
    selectedToolId: null,
    pendingRemovalId: null,
    loadingProjects: true,
    loadingTools: true,
    busy: false,
    projectError: null,
    toolsError: null,
    launchError: null,
    notice: null,
  };

  const selectedProject = () =>
    state.projects.find((project) => project.id === state.selectedProjectId) ??
    null;

  const selectedTool = () =>
    state.tools.find((tool) => tool.id === state.selectedToolId) ?? null;

  function selectPreferredTool(preferLastTool = false) {
    const currentValid = state.tools.some(
      (tool) =>
        tool.id === state.selectedToolId && tool.state.kind === "installed",
    );
    if (currentValid && !preferLastTool) return;
    const preferred = selectedProject()?.lastTool;
    if (
      preferred &&
      state.tools.some(
        (tool) => tool.id === preferred && tool.state.kind === "installed",
      )
    ) {
      state.selectedToolId = preferred;
      return;
    }
    if (currentValid) return;
    state.selectedToolId =
      state.tools.find((tool) => tool.state.kind === "installed")?.id ?? null;
  }

  function renderProjects(): HTMLElement {
    const panel = element("section", "panel projects");
    panel.setAttribute("aria-labelledby", "projects-title");

    const head = element("div", "panel-header");
    head.append(
      Object.assign(element("h2", "", "项目"), { id: "projects-title" }),
      button("＋ 添加项目", "add-project", {
        className: state.projects.length
          ? "button button-secondary"
          : "button button-primary",
        disabled: state.busy || state.loadingProjects,
      }),
    );
    panel.append(head);

    if (state.projectError) panel.append(announce(state.projectError, true));
    if (state.loadingProjects) {
      panel.append(element("p", "empty", "正在读取项目…"));
      return panel;
    }
    if (!state.projects.length) {
      const empty = element("div", "empty-state");
      empty.append(
        element("strong", "", "还没有项目"),
        element(
          "p",
          "",
          "添加一个本地文件夹，即可为它启动 Claude Code、Codex 或 Agy。",
        ),
      );
      panel.append(empty);
      return panel;
    }

    const list = element("ul", "project-list");
    for (const project of state.projects) {
      const active = state.selectedProjectId === project.id;
      const item = element(
        "li",
        "project-item" + (active ? " is-selected" : ""),
      );
      const select = button("", "select-project", {
        id: project.id,
        className: "project-choice",
        disabled: state.busy,
      });
      select.setAttribute("aria-pressed", String(active));
      select.setAttribute("aria-label", "选择项目 " + project.name);
      const content = element("span", "project-content");
      content.append(element("span", "project-name", project.name));
      const path = element("span", "project-path", project.path);
      path.title = project.path;
      content.append(path);
      select.append(
        element("span", "project-marker", active ? "●" : "○"),
        content,
      );
      item.append(select);

      if (state.pendingRemovalId === project.id) {
        const confirm = element("div", "confirm");
        confirm.append(
          element("span", "confirm-copy", "只移除记录，不删除文件夹。"),
          button("确认移除", "confirm-remove", {
            className: "button button-danger",
            disabled: state.busy,
          }),
          button("取消", "cancel-remove", {
            className: "button button-quiet",
            disabled: state.busy,
          }),
        );
        item.append(confirm);
      } else {
        const remove = button("移除", "request-remove", {
          id: project.id,
          className: "button button-quiet remove-button",
          disabled: state.busy,
        });
        remove.setAttribute("aria-label", "移除项目 " + project.name);
        item.append(remove);
      }
      list.append(item);
    }
    panel.append(list);
    return panel;
  }

  function renderTools(): HTMLElement {
    const panel = element("section", "panel tools");
    panel.setAttribute("aria-labelledby", "tools-title");

    const head = element("div", "panel-header");
    head.append(
      Object.assign(element("h2", "", "AI 工具"), { id: "tools-title" }),
      button(state.loadingTools ? "检测中…" : "重新检测", "refresh-tools", {
        className: "button button-secondary",
        disabled: state.loadingTools || state.busy,
      }),
    );
    panel.append(head);
    if (state.toolsError) panel.append(announce(state.toolsError, true));
    if (state.loadingTools && state.tools.length === 0) {
      panel.append(
        element("p", "empty", "正在检测 Claude Code、Codex 与 Agy…"),
      );
    } else {
      const list = element("ul", "tool-list");
      for (const tool of state.tools) {
        const installed = tool.state.kind === "installed";
        const active = tool.id === state.selectedToolId;
        const item = element("li", "tool-item");
        const choice = button("", "select-tool", {
          id: tool.id,
          className: "tool-choice" + (active ? " is-selected" : ""),
          disabled: !installed || state.busy,
        });
        choice.setAttribute("aria-pressed", String(active));
        choice.setAttribute("aria-label", "选择工具 " + tool.name);
        const summary = element("span", "tool-summary");
        summary.append(element("span", "tool-name", tool.name));
        const detail = element("span", "tool-detail", statusDetail(tool));
        detail.title = statusDetail(tool);
        summary.append(detail);
        const badge = element(
          "span",
          "status status-" + tool.state.kind,
          statusText(tool),
        );
        badge.prepend(
          element(
            "span",
            "status-dot",
            installed ? "●" : tool.state.kind === "missing" ? "○" : "!",
          ),
        );
        choice.append(summary, badge);
        item.append(choice);
        list.append(item);
      }
      panel.append(list);
    }
    if (state.launchError) panel.append(announce(state.launchError, true));
    if (state.notice) panel.append(announce(state.notice));
    const currentProject = selectedProject();
    const currentTool = selectedTool();
    const canStart =
      !!currentProject && currentTool?.state.kind === "installed";
    const launcher = element("div", "launcher");
    const summary = element("div", "launch-info");
    summary.append(
      element(
        "strong",
        "",
        canStart
          ? currentProject.name + " · " + currentTool.name
          : "选择项目和工具",
      ),
      element("span", "", "将在系统原生终端中运行，多个窗口可以并行使用。"),
    );
    launcher.append(
      summary,
      button(state.busy ? "启动中…" : "启动终端 ↗", "launch", {
        className: "button button-primary",
        disabled:
          !canStart ||
          state.busy ||
          state.loadingTools ||
          state.loadingProjects,
      }),
    );
    panel.append(launcher);
    return panel;
  }

  function render() {
    if (destroyed) return;
    const focused = document.activeElement;
    if (
      focused instanceof HTMLButtonElement &&
      root.contains(focused) &&
      focused.dataset.action
    ) {
      desiredFocus = { action: focused.dataset.action, id: focused.dataset.id };
    }
    const shell = element("main", "app-shell");
    const header = element("header", "topbar");
    const brand = element("div", "brand");
    brand.append(
      element("span", "brand-mark", ">_"),
      element("span", "brand-name", "AgentTerm"),
    );
    const descriptor = element("span", "tagline", "轻量 AI CLI 启动器");
    header.append(brand, descriptor);

    const intro = element("div", "intro");
    intro.append(
      element("h1", "", "一个入口，启动你的 AI 工具"),
      element(
        "p",
        "",
        "选定项目与工具，直接交给系统终端。无需内嵌终端，也无需前端框架。",
      ),
    );

    const workspace = element("div", "workspace");
    workspace.append(renderProjects(), renderTools());
    const footer = element(
      "footer",
      "footer",
      "Tauri 2 · Rust · TypeScript · Native Terminal",
    );

    shell.append(header, intro, workspace, footer);
    root.replaceChildren(shell);
    if (desiredFocus) {
      const find = (action: string, id?: string) =>
        Array.from(
          root.querySelectorAll<HTMLButtonElement>("[data-action]"),
        ).find(
          (candidate) =>
            candidate.dataset.action === action &&
            candidate.dataset.id === id &&
            !candidate.disabled,
        );
      let replacement = find(desiredFocus.action, desiredFocus.id);
      if (
        !replacement &&
        !state.busy &&
        desiredFocus.action === "request-remove" &&
        state.pendingRemovalId
      ) {
        replacement = find("cancel-remove");
      }
      if (
        !replacement &&
        !state.busy &&
        !state.pendingRemovalId &&
        ["cancel-remove", "confirm-remove"].includes(desiredFocus.action)
      ) {
        replacement =
          find("select-project", state.selectedProjectId ?? undefined) ??
          find("add-project");
      }
      if (replacement) {
        replacement.focus();
        desiredFocus = null;
      }
    }
  }

  async function loadProjects() {
    state.loadingProjects = true;
    state.projectError = null;
    render();
    try {
      state.projects = await api.listProjects();
      if (
        !state.projects.some(
          (project) => project.id === state.selectedProjectId,
        )
      ) {
        state.selectedProjectId = state.projects[0]?.id ?? null;
      }
      selectPreferredTool(true);
    } catch (error) {
      state.projectError = "无法读取项目列表：" + describeError(error);
    } finally {
      state.loadingProjects = false;
      render();
    }
  }

  async function refreshTools() {
    state.loadingTools = true;
    state.toolsError = null;
    render();
    try {
      state.tools = await api.detectTools();
      selectPreferredTool();
    } catch (error) {
      state.toolsError = "无法读取工具状态：" + describeError(error);
    } finally {
      state.loadingTools = false;
      render();
    }
  }

  async function addProject() {
    state.busy = true;
    state.projectError = null;
    render();
    try {
      const path = await api.pickDirectory();
      if (!path) return;
      const project = await api.addProject(path);
      state.projects.push(project);
      state.selectedProjectId = project.id;
      state.pendingRemovalId = null;
      selectPreferredTool(true);
    } catch (error) {
      state.projectError = "无法添加项目：" + describeError(error);
    } finally {
      state.busy = false;
      render();
    }
  }

  async function confirmRemove() {
    const id = state.pendingRemovalId;
    if (!id) return;
    state.busy = true;
    state.projectError = null;
    render();
    try {
      await api.removeProject(id);
      state.projects = state.projects.filter((project) => project.id !== id);
      if (state.selectedProjectId === id)
        state.selectedProjectId = state.projects[0]?.id ?? null;
      state.pendingRemovalId = null;
      selectPreferredTool(true);
    } catch (error) {
      state.projectError = "无法移除项目：" + describeError(error);
    } finally {
      state.busy = false;
      render();
    }
  }

  async function launch() {
    const project = selectedProject();
    const tool = selectedTool();
    if (!project || !tool || tool.state.kind !== "installed") return;
    state.busy = true;
    state.launchError = null;
    state.notice = null;
    render();
    try {
      const result = await api.launchNativeTool(tool.id, project.path);
      if (!result.launched) throw new Error("系统终端未报告启动成功");
      if (!result.warning) project.lastTool = tool.id;
      state.notice = result.warning
        ? "已在系统终端启动 " + tool.name + "，但" + result.warning
        : "已请求在系统终端启动 " + tool.name + "。";
    } catch (error) {
      state.launchError = "无法启动终端：" + describeError(error);
    } finally {
      state.busy = false;
      render();
    }
  }

  function handleClick(event: MouseEvent) {
    const target =
      event.target instanceof Element
        ? event.target.closest<HTMLButtonElement>("[data-action]")
        : null;
    if (!target || !root.contains(target) || target.disabled) return;
    const id = target.dataset.id;
    if (target.dataset.action)
      desiredFocus = { action: target.dataset.action, id };
    switch (target.dataset.action) {
      case "add-project":
        void addProject();
        break;
      case "refresh-tools":
        void refreshTools();
        break;
      case "select-project":
        if (id) {
          state.selectedProjectId = id;
          state.pendingRemovalId = null;
          state.launchError = null;
          state.notice = null;
          selectPreferredTool(true);
          render();
        }
        break;
      case "select-tool":
        if (
          id &&
          state.tools.some(
            (tool) => tool.id === id && tool.state.kind === "installed",
          )
        ) {
          state.selectedToolId = id;
          state.launchError = null;
          state.notice = null;
          render();
        }
        break;
      case "request-remove":
        if (id) {
          state.pendingRemovalId = id;
          render();
        }
        break;
      case "confirm-remove":
        void confirmRemove();
        break;
      case "cancel-remove":
        state.pendingRemovalId = null;
        render();
        break;
      case "launch":
        void launch();
        break;
    }
  }

  root.addEventListener("click", handleClick);
  render();
  const ready = Promise.all([loadProjects(), refreshTools()]).then(
    () => undefined,
  );

  return {
    ready,
    destroy() {
      destroyed = true;
      desiredFocus = null;
      root.removeEventListener("click", handleClick);
      root.replaceChildren();
    },
  };
}
