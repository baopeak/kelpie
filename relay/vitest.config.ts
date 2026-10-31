import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    cloudflareTest({
      main: "./test/entry.ts",
      miniflare: {
        compatibilityDate: "2026-10-15",
        durableObjects: { ROOM: { className: "Room", useSQLite: true } },
        bindings: { TICKET_SECRET: "test-only-ticket-secret", RELAY_VERSION: "test" },
        // The room asks Cloudflare where it runs; tests must not reach the network.
        outboundService: () => new Response("fl=1\ncolo=TST\n"),
      },
    }),
  ],
  test: { include: ["test/**/*.test.ts"] },
});
