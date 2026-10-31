// The pictures a composer holds until they are sent, shared by the chat's message field and the
// floating composer: pasted or dropped, the limits on them, and their bytes for a chat.
import { createSignal } from "solid-js";
import { IMAGE_EXT, IMAGE_MIME, rememberAttached } from "../chat/chat-images";
import type { ChatImage } from "../chat/chat-model";
import { errorText, t } from "./i18n";
import { readImage, writeTempFile } from "./ipc";
import { basename } from "./platform";
import { store } from "./store";
import type { SessionInfo } from "./types";

export interface Attached { id: number; name: string; mime: string; path: string; /** Base64 bytes: kept for chat views, which take the picture itself. */ data?: string; url?: string }

const MAX_IMAGES = 8;
/** A picture a chat sends to the model: the API refuses more than 5 MB. */
const CHAT_IMAGE_MAX = 5 * 1024 * 1024;
/** The base64 of one message's pictures, under `kelpie_proto::MAX_FRAME_LEN` with room for its JSON:
 *  the daemon hangs up on a longer frame, which ends the connection for every session. */
const CHAT_MESSAGE_MAX = 30 * 1024 * 1024;

const readAsBase64 = (file: File) =>
  new Promise<string>((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).split(",")[1] ?? "");
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });

const base64Bytes = (data: string) => Math.floor((data.length * 3) / 4);

const decodes = (file: File) =>
  createImageBitmap(file).then(
    (bitmap) => (bitmap.close(), true),
    () => false,
  );

/** One composer's pictures. `onOther` gets what a drop held besides pictures, once the drop is through. */
export function createPictures(session: () => SessionInfo | undefined, onOther?: (paths: string[]) => void) {
  const [images, setImages] = createSignal<Attached[]>([]);
  let attachedId = 0;
  const chat = () => !!session()?.chat;

  /** One at a time, in the order they came: each is checked against the limit once those before it are in. */
  let last: Promise<unknown> = Promise.resolve();
  const inTurn = <T,>(work: () => Promise<T>): Promise<T> => {
    const run = last.then(work);
    last = run.catch(() => {});
    return run;
  };

  const add = (picture: Omit<Attached, "id">) => {
    rememberAttached(picture.path);
    setImages((l) => [...l, { ...picture, id: ++attachedId }]);
  };

  async function attach(file: File) {
    const ext = IMAGE_EXT[file.type];
    if (!ext) return;
    if (images().length >= MAX_IMAGES) return store.notify(t("At most {n} pictures", { n: MAX_IMAGES }));
    if (session()?.remote) return store.notify(t("Pictures cannot be sent to a session on another machine"));
    if (chat() && file.size > CHAT_IMAGE_MAX) return store.notify(t("A picture sent to a chat can be at most 5 MB"), undefined, 6000);
    try {
      // A file that only claims to be a picture: the model refuses it, and that request stays in the
      // conversation's history, where it can fail every turn after it.
      if (typeof createImageBitmap === "function" && !(await decodes(file))) return store.notify(t("{name} is not a picture that can be read", { name: file.name }), undefined, 6000);
      const data = await readAsBase64(file);
      const path = await writeTempFile("image", ext, data);
      add({ name: basename(path), mime: file.type, path, data, url: `data:${file.type};base64,${data}` });
    } catch (e) {
      store.notify(errorText(e), undefined, 6000);
    }
  }

  /** A dropped file: a picture is attached by its own path, anything else goes to `onOther`. */
  async function attachDropped(paths: string[]) {
    const others: string[] = [];
    for (const p of paths) {
      const mime = IMAGE_MIME[p.split(".").pop()?.toLowerCase() ?? ""];
      if (!mime) {
        others.push(p);
        continue;
      }
      if (images().length >= MAX_IMAGES) return store.notify(t("At most {n} pictures", { n: MAX_IMAGES }));
      if (session()?.remote) return store.notify(t("Pictures cannot be sent to a session on another machine"));
      try {
        const data = chat() ? (await readImage(p)).base64 : undefined;
        if (data && base64Bytes(data) > CHAT_IMAGE_MAX) {
          store.notify(t("A picture sent to a chat can be at most 5 MB"), undefined, 6000);
          continue;
        }
        add({ name: basename(p), mime, path: p, data, url: data ? `data:${mime};base64,${data}` : undefined });
      } catch (e) {
        store.notify(errorText(e), undefined, 6000);
      }
    }
    onOther?.(others);
  }

  /** A paste of pictures attaches them. Text is left to the field, also with a picture beside it:
   *  Office puts a rendering of what was copied next to its text, and the text is what was meant. */
  function paste(e: ClipboardEvent) {
    const pictures = [...(e.clipboardData?.files ?? [])].filter((f) => f.type in IMAGE_EXT);
    if (!pictures.length || e.clipboardData?.getData("text/plain").trim()) return;
    e.preventDefault();
    for (const f of pictures) void inTurn(() => attach(f));
  }

  /** What a chat is sent. The bytes of a picture dropped while the session was a terminal are read now.
   *  Refused, nothing is lost: the pictures stay until `forget` is told they went. */
  async function forChat(): Promise<ChatImage[]> {
    const list = await Promise.all(
      images().map(async (i) => {
        const data = i.data ?? (await readImage(i.path)).base64;
        if (base64Bytes(data) > CHAT_IMAGE_MAX) throw new Error(t("A picture sent to a chat can be at most 5 MB"));
        return { mime: i.mime, data, path: i.path };
      }),
    );
    if (list.reduce((sum, i) => sum + i.data.length, 0) > CHAT_MESSAGE_MAX) throw new Error(t("Too many pictures for one message: send them in smaller groups"));
    return list;
  }

  return {
    images,
    drop: (paths: string[]) => inTurn(() => attachDropped(paths)),
    paste,
    forChat,
    remove: (id: number) => setImages((l) => l.filter((x) => x.id !== id)),
    forget: (sent: Attached[]) => setImages((l) => l.filter((x) => !sent.includes(x))),
  };
}
