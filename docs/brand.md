# Kelpie brand

Kelpie is named after the Australian herding dog that rounds up a whole flock, and the Scottish water spirit (a "daemon"): one app that keeps every coding agent together and running in the background.

Assets live in [app/src/assets/brand/](../app/src/assets/brand/); the design canvas is https://claude.ai/artifact/F3idpWC6i9kent9G1Gu7dt.

## Logo

| File | Use |
|---|---|
| `kelpie-mark.svg` / `kelpie-mark-dark.svg` | The mark: two ears behind a `>_` window. Light / dark grounds. |
| `kelpie-wordmark.svg` / `-light.svg` | "kelpie" in Bricolage Grotesque ExtraBold, outlined. |
| `kelpie-lockup.svg` / `-light.svg` | Mark + wordmark, horizontal. |
| `kelpie-app-icon.svg` | App icon: Kip peeking over a terminal band on Night. Source of `app/src-tauri/icons/`. |

- Clear space around the logo: at least the width of the cursor bar.
- Minimum size: 16 px for the mark, 96 px wide for the lockup.
- Do not rotate, stretch, recolor outside the palette or add effects.

## Color

| Token | Hex | Use |
|---|---|---|
| Kelpie Rust | `#E8743B` | Primary accent, the dog. Text on light: `#B4501F`. |
| Night | `#141823` | Main text, dark grounds, terminal. |
| Spirit Teal | `#18A999` | Cursor, secondary accent. Text on light: `#0F7C70`. |
| Tan | `#F6A971` | Details, paws. |
| Cream | `#FFF1E2` | Text on dark, muzzle. |
| Paper | `#F6F6F3` | Light grounds. |

Agent status: working `#18A999`, needs you `#F5B53F`, done `#3FAE6A`, idle `#8A91A3`.

## Agent logos

Sessions of a known agent CLI (Claude Code, Codex, Gemini, Cursor, OpenCode, Copilot) show that
tool's own mark, unaltered, on a neutral tile; other kinds keep a lettered tile in their hue. The
marks belong to their owners and never appear in Kelpie's own branding. Sources, licence and the
usage rules: [third-party-agent-logos.md](third-party-agent-logos.md).

## Type

- **Bricolage Grotesque** 800: wordmark and display headings.
- **Inter** 400–600: interface and body.
- **JetBrains Mono** 700: terminal and code.

## Mascot: Kip

A kelpie peeking out of a terminal window. One file per agent state:

| File | State |
|---|---|
| `kip-peek.svg` | Default, welcome, empty states |
| `kip-working.svg` | Agent working |
| `kip-needs.svg` | Agent needs you |
| `kip-done.svg` | Agent done |
| `kip-sleep.svg` | Idle / hibernated |

In the app (`app/src/components/brand.tsx`, `kip.css`), Kip peeks over the sidebar footer and
follows the agents: one needs you, they work, one finished, all quiet, or a nap after 10 quiet
minutes. Clicking Kip opens the session it reacts to. Kip also greets you on empty screens (no
project, a project without tabs, an empty pane, an empty inbox).

The SVG files mark the parts that move (`kip-ear-l`, `kip-ear-r`, `kip-eyes`); the app inlines
them. Motion has to stay light, because Kelpie must cost almost nothing while agents run:

- Nothing loops. A mood plays its entry once; after that a single timer adds one short gesture
  (blink, ear twitch, bob, nudge, hop, breath) every few seconds, and nothing runs in between. A
  looping animation, however small, made the webview redraw the window every frame: about 18%
  of a core while agents worked, against about 3% with gestures (`scripts/e2e/mascot.py`
  measures it).
- No gestures while the window is hidden or another app has the focus.
- Settings → Appearance: **Kip, the mascot** (off: no Kip and no timers at all) and **Kip
  animations** (Always / Follow system / Off; Follow system honours the OS's animation setting).

## Voice

Clear, short and warm, like a teammate: say what happened and what to do next.
