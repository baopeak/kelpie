---
phase: 8
title: "Phone Files tab and Settings → Mobile → Files"
status: pending
priority: P2
effort: "3d"
dependencies: [7]
---

# Phase 8: Phone files UI and desktop settings

## Overview
- **Phone (installed PWA)**: a *Files* tab to browse the allowed roots, download and save (Files or Photos on iOS, Downloads on Android), and upload photos and files, with progress, cancel and resume.
- **Desktop**: Settings → Mobile gets a *Files from the phone* row, with roots, hosts and caps.

## Context
- **The wire**: `app/phone/src/link.ts` is the only file that knows it. It declares `caps` (`link.ts:605-606`), and sends and settles requests by `req` (`:495-507,886-888`).
- **Views**: inbox ↔ session view, chosen in `app/phone/src/app.tsx:218-222`.
- **Cache**: IndexedDB, as `storage.ts`/`cache.ts` already use.
- **Settings rows**: `option-row.tsx` (phone phase 3d), with presence for widening (`mobileSecure`, `app/src/lib/ipc.ts:14`).
- **PWA facts** (report sec 4):
  - `navigator.share({files})` needs a user gesture and HTTPS; gate it with `canShare`;
  - `showSaveFilePicker` is not on iOS and is unreliable on Android (feature-detect);
  - `<a download>` with a Blob is unreliable in iOS standalone mode;
  - `<input type=file multiple>` gives Photo Library, Take Photo and Browse; avoid `accept="*/*"` (WebKit 229456);
  - iOS holds the Blob in memory.
- **Bundle guard**: the phone bundle may not import `@tauri-apps/*` or `lib/ipc` (`docs/architecture.md:696`).

## Requirements
### Phone
- **Tab**: shown when `hello.files.on`. Header switch *Sessions | Files*.
- **Browse**:
  - roots list, then folder list (name, size, date, sorted folders first), breadcrumbs, pull to refresh, and *Load more* for paged lists;
  - host roots show the host label;
  - offline: the last listing, dimmed, with "as of" (the same pattern as the inbox).
- **Download**:
  1. Tap a file to see its size, then *Download*. A file over this platform's cap is refused before any byte, with "too big for the phone: use the computer".
  2. Progress, speed and cancel. The parts go to IndexedDB in 1 MiB Blobs, so memory stays low and a reload or kill can resume (within 24 h; offset = stored length).
  3. When done, check the SHA-256 against `fs_done` (incremental, `@noble/hashes`, already used by `noise.ts`).
  4. *Save* button (a second tap, which is the gesture the share sheet needs):
     - iOS: `navigator.share({files: [file]})` if `canShare`;
     - else `showSaveFilePicker` if present;
     - else (Android) `<a download>` with an object URL;
     - last resort (iOS without file share): open the object URL in a new view with "press and hold, then Save".
  5. The parts are deleted after a save or after 24 h.
- **Upload**:
  1. *Upload here* (write devices, in a folder). `<input type=file multiple>` with no `accept`, so photos and files both work. iOS may hand over HEIC as JPEG: the name follows what the browser gives.
  2. Per file: `fs_put`; on `exists`, ask *Keep both* / *Replace* / *Skip* (with "apply to all").
  3. Streamed hashing while sending; at most 16 chunks in flight; pacing over the relay is the daemon's (it acks).
  4. Resume while the page lives: reconnect gives `fs_put` the same `xid`, which returns the offset. After a reload the person picks the file again; a match on name, size and `lastModified` offers *Continue*. A prefix that differs is caught by the final SHA-256 (`corrupt` → start over).
- **Transfers sheet**: in-flight and recent transfers, with cancel, retry and save (for finished downloads still stored).
- **States**: a read-only device has no upload button, with the hint "can only read". `relay_budget`, `too_big_for_relay`, `changed` and `locked` are each one plain sentence. The page hidden pauses; the page shown resumes.
- **Wire**: data messages are encoded and decoded only in `link.ts`. A `files` cap is added to `caps`.
- **Strings**: in `app/phone` through the existing dictionary (VI from `app/src/locales/vi/phone.json`).

### Desktop
Settings → Mobile → *Files from the phone*. An `option-row` with three plain lines:
- what it does;
- the trade-off: the phone can read what is in these folders; uploads land in the upload folder or the folder you allow;
- how safe it is: only phones with Face ID / passkey lock over HTTPS; everything is in the audit log.

The row's controls:
- **Folders**: the upload folder (changeable), plus *Add folder…* and *Add the open projects' folders*. Each folder can be removed.
- **Hosts**: tick saved hosts.
- **Caps**: relay per-file and monthly, and the phone size caps, with the month's relay use shown.
- **Presence**: adding or raising goes through `mobileSecure("mobile.set_files")`; removing or lowering goes through `daemon()`.
- **Search**: through `settings-catalog.ts`.

## Files
- Create: `app/phone/src/files.tsx` (tab, browser, transfers sheet), `app/phone/src/files-store.ts` (state, IndexedDB parts, resume), `app/phone/src/file-save.ts` (save-path choice), and their tests `files-store.test.ts`, `file-save.test.ts`; `app/src/components/settings/phone-files-settings.tsx`.
- Modify:
  - `app/phone/src/link.ts` (`files` cap, data codec, credit `fs_ack`);
  - `app/phone/src/app.tsx` (tab switch);
  - `app/phone/src/phone-state.ts`;
  - `app/src/locales/vi/phone.json`, `app/src/locales/vi/settings.json`;
  - `app/src/components/settings/mobile-settings.tsx`, `app/src/lib/settings-catalog.ts`, `app/src/lib/ipc.ts` (`mobileSecure` union);
  - `app/src/styles.css`, the phone styles;
  - `scripts/e2e/mobile.py` (or a new GUI scenario);
  - `docs/user-guide.md` (phone section: Files), `docs/architecture.md` (phone client: files).

## Steps
1. `link.ts` codec and credit, with tests (vectors shared with the Rust codec test).
2. `files-store.ts`: IndexedDB parts, resume, hashing; `file-save.ts`: platform choice, with tests.
3. `files.tsx`: browse, download, save, upload, transfers sheet.
4. Desktop settings row with presence; catalog entry.
5. Guide and architecture. Manual device pass.

## Tests
- Vitest (`pnpm exec vitest run app/phone/src/files-store.test.ts app/phone/src/file-save.test.ts app/phone/src/link.test.ts`):
  - **codec**: the data codec matches the Rust vectors; credit window never exceeded;
  - **reassembly**: in order; a gap is an error;
  - **resume**: the offset equals the stored length; parts are deleted after a save and after 24 h;
  - **save path**: chosen for each environment (iOS with or without `canShare` files, Android, `showSaveFilePicker` present);
  - **caps**: refusal before the first byte;
  - **upload match**: by name, size and `lastModified`;
  - **refusals**: each code maps to its sentence.
- `pnpm typecheck && pnpm test && pnpm i18n:check && pnpm build:phone` (bundle guard and size noted against the ~83 KB gzip figure, `docs/architecture.md:697`).
- e2e:
  - `scripts/e2e/mobile_files.py` extended: drive the real phone page in an emulated mobile browser (the project's emulated-browser pattern) on the isolated daemon. Browse, download a 5 MB file (hash matches, the save path picked for the emulated platform), upload 3 files, and resume after a forced reconnect.
  - Desktop GUI: turn the row on (test presence seam), add a folder, and check `mobile.status` on that endpoint.
- **Manual (the only part that needs a person)**, on an iOS installed PWA over Tailscale:
  - save a 50 MB video to Files and to Photos through the share sheet;
  - upload 10 photos from the library;
  - kill the app mid-download and reopen it, and it resumes;
  - find the largest file that saves without a crash, then set the iOS cap from it.
  Then Android Chrome PWA: Downloads.

## Risk
| Risk | L × I | Mitigation |
|---|---|---|
| iOS share sheet refuses some types or big files | M × M | `canShare` check; blob-open fallback; cap tuned on a device |
| iOS kills the PWA in the background | H × L | Parts in IndexedDB, resume on show |
| Memory spike when building the final File from parts | M × M | Blob of Blobs (lazy); cap; measured on a device |
| Settings row widens reach without presence | L × H | Daemon refuses `Role::User` widening (phase 7 tests); the row uses `mobileSecure` |
| Bundle grows | L × L | Files UI is loaded on first open (dynamic import), like the adapters |

**Rollback**: revert. The tab disappears; the daemon verbs remain but `files` stays off; IndexedDB parts expire after 24 h.
