import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    cloudflareTest({
      main: "./test/entry.ts",
      miniflare: {
        compatibilityDate: "2026-10-15",
        durableObjects: { ROOM: { className: "Room", useSQLite: true } },
        bindings: { TICKET_SECRET: "test-only-ticket-secret", REGISTRATION_KEY: "test-only-registration-key", RELAY_VERSION: "test" },
        // Tests must not reach the network.
        outboundService: () => new Response("not in tests\n", { status: 599 }),
      },
    }),
  ],
  test: { include: ["test/**/*.test.ts"] },
});
