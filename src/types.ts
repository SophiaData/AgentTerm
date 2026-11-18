export type Project = {
  id: string;
  name: string;
  path: string;
  lastTool?: string | null;
};

export type ToolState =
  | { kind: "installed"; path: string; version: string | null }
  | { kind: "missing" }
  | { kind: "notExecutable"; reason: string };

export type ToolStatus = {
  id: string;
  name: string;
  state: ToolState;
};

export function statusText(tool: ToolStatus): string {
  switch (tool.state.kind) {
    case "installed":
      return tool.state.version ? "已安装 · " + tool.state.version : "已安装";
    case "missing":
      return "未安装";
    case "notExecutable":
      return "不可执行";
  }
}

export function statusDetail(tool: ToolStatus): string {
  switch (tool.state.kind) {
    case "installed":
      return tool.state.path;
    case "missing":
      return "未在 PATH 或常见安装目录中找到";
    case "notExecutable":
      return tool.state.reason;
  }
}
