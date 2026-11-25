import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [cloudflareTest({ wrangler: { configPath: "./wrangler.jsonc" } })],
  define: { __BIG__: JSON.stringify(Boolean(process.env.RELAY_1GB)) },
  test: { testTimeout: 20_000 },
});
