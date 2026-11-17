export const PRODUCT_NAME = "AgentTerm";

export const SUPPORTED_TARGETS = [
  { id: "macos-arm64", label: "macOS · Apple Silicon" },
  { id: "windows-x64", label: "Windows · x64" },
] as const;

export type SupportedTargetId = (typeof SUPPORTED_TARGETS)[number]["id"];

export function isSupportedTarget(target: string): target is SupportedTargetId {
  return SUPPORTED_TARGETS.some((item) => item.id === target);
}
