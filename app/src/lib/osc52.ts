// OSC 52: a program asks the terminal to put text on the clipboard (`ESC ] 52 ; c ; <base64>`).
// Claude Code's fullscreen mode copies its own mouse selection this way. Only writes are taken;
// a query (`?`) would let any program read the clipboard and is never answered.

/** Largest text a program may put on the clipboard at once. */
const MAX_BYTES = 1 << 20;

/** The text an OSC 52 payload (`<selection>;<base64>`) sets, or null for a query, a clear or
 *  anything malformed. */
export function osc52Text(data: string): string | null {
  const sep = data.indexOf(";");
  if (sep < 0) return null;
  const payload = data.slice(sep + 1);
  if (!payload || payload === "?" || payload.length > (MAX_BYTES * 4) / 3 + 4) return null;
  try {
    const bytes = Uint8Array.from(atob(payload), (c) => c.charCodeAt(0));
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}
