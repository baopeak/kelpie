import { defineConfig } from "vitest/config";

// Unit tests cover the pure TS modules (layout maths, agent adapters); anything that needs the
// webview is covered by the WebDriver scripts in scripts/e2e. The "browser" condition makes
// solid-js resolve to its client build, as in the app.
export default defineConfig({
  resolve: { conditions: ["browser"] },
  test: { include: ["src/**/*.test.ts"], environment: "node" },
});
