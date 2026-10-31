// From a picture in the transcript to something an `<img>` can show. A path in the log or in history is
// the agent's word for it, so what may be read is decided here, before any file-system call: what was
// attached in this window, or a file directly in the composer's own temp folder. Anything else stays a
// name. Not for the phone: it reads no files.
import { tempDir } from "@tauri-apps/api/path";
import { readImage } from "../lib/ipc";
import type { UserImage } from "./chat-model";

export const IMAGE_EXT: Record<string, string> = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp", "image/bmp": "bmp" };
export const IMAGE_MIME: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", bmp: "image/bmp" };

const PICTURE_TYPES = new Set(Object.values(IMAGE_MIME));
/** A thumbnail is the whole picture as a data URL, so the cache's limit is in bytes. */
const CACHE_MAX_BYTES = 64 * 1024 * 1024;
/** The composer's temp folder, under the system's (`sweep_temp` empties it after a few days). */
const COMPOSER_FOLDER = "kelpie-composer";

/** Paths the person attached in this window: they may lie anywhere, and were chosen by them. */
const attached = new Set<string>();

export const rememberAttached = (path: string) => void attached.add(path);

/** A file directly in the folder, by the letter: nothing to walk out through (`..`, a stream's `:`). */
function inComposerFolder(path: string, temp: string): boolean {
  const slashes = (p: string) => p.replace(/\\/g, "/");
  const folder = `${slashes(temp).replace(/\/+$/, "")}/${COMPOSER_FOLDER}/`;
  const file = slashes(path);
  // A drive letter: this file system does not tell case apart.
  const same = /^[a-z]:/i.test(folder) ? file.slice(0, folder.length).toLowerCase() === folder.toLowerCase() : file.startsWith(folder);
  return same && /^[^/:]+$/.test(file.slice(folder.length));
}

const dataUrl = (mime: string, base64: string) => `data:${mime};base64,${base64}`;

export function createImageUrl(io: { read: (path: string) => Promise<{ mime: string; base64: string }>; tempDir: () => Promise<string> }) {
  /** By path, the newest last. Holds the read itself, so a second thumbnail does not read again. */
  const cache = new Map<string, { url: Promise<string | null>; bytes: number }>();
  let held = 0;

  const mayRead = async (path: string) => attached.has(path) || inComposerFolder(path, await io.tempDir());

  const load = async (path: string) => {
    if (!(await mayRead(path))) return null;
    const { mime, base64 } = await io.read(path);
    return PICTURE_TYPES.has(mime) ? dataUrl(mime, base64) : null;
  };

  function cached(path: string): Promise<string | null> {
    const hit = cache.get(path);
    if (hit) {
      cache.delete(path);
      cache.set(path, hit);
      return hit.url;
    }
    const entry = { url: load(path).catch(() => null), bytes: 0 };
    cache.set(path, entry);
    entry.url = entry.url.then((url) => {
      // A failure is not remembered: the next look may find the file, or be allowed it.
      if (url === null) cache.delete(path);
      else remember(path, entry, url.length);
      return url;
    });
    return entry.url;
  }

  /** Counts a finished read, and lets go of the oldest beyond the limit, never this one. */
  function remember(path: string, entry: { bytes: number }, bytes: number) {
    entry.bytes = bytes;
    held += bytes;
    for (const [other, { bytes: size }] of cache) {
      if (held <= CACHE_MAX_BYTES) break;
      if (other === path) continue;
      cache.delete(other);
      held -= size;
    }
  }

  /** The picture's URL, or null for a named placeholder. */
  return async function imageUrl(img: UserImage): Promise<string | null> {
    if (img.mime != null && !PICTURE_TYPES.has(img.mime)) return null;
    if (img.data != null) return img.mime ? dataUrl(img.mime, img.data) : null;
    return img.path ? cached(img.path) : null;
  };
}

export const imageUrl = createImageUrl({ read: readImage, tempDir });
