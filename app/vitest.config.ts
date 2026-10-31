import { defineConfig } from "vitest/config";

// Unit tests cover the pure TS modules (layout maths, agent adapters); anything that needs the
// webview is covered by the WebDriver scripts in scripts/e2e. The "browser" condition makes
// solid-js resolve to its client build, as in the app; tests run in Vite's server environment, which
// takes its conditions from `ssr`. Under the server build memos and effects would not react.
export default defineConfig({
  resolve: { conditions: ["browser"] },
  ssr: { resolve: { conditions: ["browser"] } },
  test: { include: ["src/**/*.test.ts", "phone/src/**/*.test.ts"], environment: "node" },
});
