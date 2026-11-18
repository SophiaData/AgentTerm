/** Minimal DOM helpers; untrusted project names/paths are always text, never HTML. */
export function element<K extends keyof HTMLElementTagNameMap>(
  name: K,
  className = "",
  text = "",
): HTMLElementTagNameMap[K] {
  const result = document.createElement(name);
  if (className) result.className = className;
  if (text) result.textContent = text;
  return result;
}

export function button(
  title: string,
  action: string,
  options: { id?: string; disabled?: boolean; className?: string } = {},
): HTMLButtonElement {
  const result = element("button", options.className ?? "button", title);
  result.type = "button";
  result.dataset.action = action;
  if (options.id) result.dataset.id = options.id;
  result.disabled = options.disabled ?? false;
  return result;
}

export function announce(text: string, error = false): HTMLParagraphElement {
  const result = element(
    "p",
    error ? "message message-error" : "message",
    text,
  );
  result.setAttribute("role", error ? "alert" : "status");
  if (!error) result.setAttribute("aria-live", "polite");
  return result;
}
