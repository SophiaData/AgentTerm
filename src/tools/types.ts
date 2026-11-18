export type ToolState =
  | { kind: "installed"; path: string; version: string | null }
  | { kind: "missing" }
  | { kind: "notExecutable"; reason: string };

export type ToolStatus = {
  id: string;
  name: string;
  state: ToolState;
};

export function badgeLabel(state: ToolState): string {
  switch (state.kind) {
    case "installed":
      return "已安装";
    case "missing":
      return "未安装";
    case "notExecutable":
      return "无法执行";
  }
}

export function detailText(state: ToolState): string {
  switch (state.kind) {
    case "installed":
      return state.version ? `版本 ${state.version}` : `已找到：${state.path}`;
    case "missing":
      return "未在 PATH 或常见安装目录中找到";
    case "notExecutable":
      return state.reason;
  }
}
