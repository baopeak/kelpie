import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    cloudflareTest({
      main: "./test/entry.ts",
      miniflare: {
        compatibilityDate: "2026-10-15",
        durableObjects: { ROOM: { className: "Room", useSQLite: true }, REGISTRAR: { className: "Registrar", useSQLite: true } },
        // Key mode, as the relays before open registration; tests of open registration pass their own env. No rate binding: where there is
        // none (workerd) it is skipped, and tests give a fake one.
        bindings: { TICKET_SECRET: "test-only-ticket-secret", REGISTRATION_KEY: "test-only-registration-key", RELAY_VERSION: "test" },
        // Tests must not reach the network.
        outboundService: () => new Response("not in tests\n", { status: 599 }),
      },
    }),
  ],
  test: { include: ["test/**/*.test.ts"] },
});
