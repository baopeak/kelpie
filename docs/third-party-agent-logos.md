# Agent logos

Kelpie shows the mark of each agent CLI it can launch next to that session, so a session's tool is
recognisable at a glance. The marks are only used to say which tool a session runs. Kelpie is not
affiliated with, sponsored or endorsed by any of their owners.

| Kind | Mark | Owner | Colour |
|---|---|---|---|
| `claude` | Claude spark | Anthropic, PBC | `#D97757` |
| `codex` | OpenAI Blossom | OpenAI | text colour (black / white) |
| `gemini` | Gemini spark | Google LLC | four-colour, as published |
| `cursor` | Cursor cube | Anysphere, Inc. | text colour |
| `opencode` | OpenCode mark | OpenCode | text colour |
| `copilot` | Copilot icon | GitHub, Inc. | text colour |

Claude, Claude Code, OpenAI, Codex, Gemini, Cursor, OpenCode, GitHub and GitHub Copilot are
trademarks of their respective owners.

## Rules the app follows

These come from the owners' published guidelines ([Anthropic](https://www.anthropic.com/legal/trademark-guidelines),
[GitHub](https://brand.github.com/foundations/logo), [Google](https://partnermarketinghub.withgoogle.com/brands/google/use-cases/product-co-branding/))
and the practice of the ACP agent registry, Zed and VS Code:

- A mark is never recoloured, stretched, cropped or combined with other artwork. It is drawn in
  its owner's colour, or in plain black / white for single-colour marks, on a neutral tile with
  clear contrast in both themes (`.kind-glyph.logo` in `app/src/styles.css`).
- A mark always sits next to the tool's name, or carries it as its accessible label; it is never
  Kelpie's own icon and never used in Kelpie's branding.
- An ended session fades its mark (opacity) instead of greying it.
- Kinds without a mark here (Terminal, Command, OMP, other presets) keep a lettered colour tile.

## Source

Path data is copied unchanged from [Lobe Icons](https://github.com/lobehub/lobe-icons)
(`@lobehub/icons-static-svg` 1.98.0) into `app/src/lib/agent-logos.ts`. Their licence covers the
artwork files only and grants no trademark rights:

```
MIT License

Copyright (c) 2023 LobeHub

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Update: take the new `icons/<name>.svg` (colour variant for Claude and Gemini) from the same
package, copy its path data, bump the version above, and check the owner's guidelines again.
