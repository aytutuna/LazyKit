import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [cloudflareTest({
      wrangler: { configPath: "./wrangler.jsonc" },
      miniflare: { compatibilityFlags: ["web_socket_manual_reply_to_close"], bindings: { ACCOUNT_KEY: "test-account-key", ID_KEY: "test-id-key", PASSWORD_STAGES: "1", GOOGLE_CLIENT_IDS: "test-google-client" } },
    })],
  define: { __BIG__: JSON.stringify(Boolean(process.env.RELAY_1GB)) },
  test: { testTimeout: 20_000 },
});
