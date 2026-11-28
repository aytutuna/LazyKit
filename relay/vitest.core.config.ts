import { defineConfig } from "vitest/config";

// The sign-in core with its in-memory fakes, on plain Node: nothing here may touch Cloudflare.
export default defineConfig({ test: { include: ["test/core/**/*.test.ts"], environment: "node" } });
