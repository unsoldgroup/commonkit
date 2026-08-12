/** Render persisted or remote text through the DOM text boundary, never HTML. */
export function setTextContent(element: { textContent: string }, value: unknown) {
  element.textContent = String(value ?? "");
}
