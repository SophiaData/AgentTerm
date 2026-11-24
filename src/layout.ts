export const LAYOUT_STORAGE_KEY = "agentterm-layout";

export type StoredLayout = {
  sidebarWidth: number;
  sideWidth: number;
  filesOpen: boolean;
  projectId: string | null;
};

function boundedNumber(value: unknown, minimum: number, maximum: number) {
  return typeof value === "number" &&
    Number.isFinite(value) &&
    value >= minimum &&
    value <= maximum
    ? value
    : null;
}

export function parseStoredLayout(raw: string | null): StoredLayout | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as Record<string, unknown>;
    if (!value || value.version !== 2) return null;
    const sidebarWidth = boundedNumber(value.sidebarWidth, 120, 600);
    const sideWidth = boundedNumber(value.sideWidth, 240, 900);
    if (
      sidebarWidth === null ||
      sideWidth === null ||
      typeof value.filesOpen !== "boolean" ||
      (value.projectId !== null && typeof value.projectId !== "string")
    ) {
      return null;
    }
    return {
      sidebarWidth,
      sideWidth,
      filesOpen: value.filesOpen,
      projectId: value.projectId as string | null,
    };
  } catch {
    return null;
  }
}

export function loadStoredLayout(): StoredLayout | null {
  try {
    const raw = localStorage.getItem(LAYOUT_STORAGE_KEY);
    const parsed = parseStoredLayout(raw);
    if (raw && !parsed) localStorage.removeItem(LAYOUT_STORAGE_KEY);
    return parsed;
  } catch {
    return null;
  }
}

export function saveStoredLayout(layout: StoredLayout): void {
  try {
    localStorage.setItem(
      LAYOUT_STORAGE_KEY,
      JSON.stringify({ version: 2, ...layout }),
    );
  } catch {
    // Layout persistence is optional; the current in-memory layout still works.
  }
}
