import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  test: {
    // 只跑 src 里的用例；包根的 `eslint-guard.test.js` 是 node:test 写的 lint 反向夹具，归 `npm run lint`。
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
