import { defineConfig } from "vitest/config";

// Only the TypeScript sources: `npm run build` also emits compiled tests into dist/.
export default defineConfig({ test: { include: ["src/**/*.test.ts"] } });
