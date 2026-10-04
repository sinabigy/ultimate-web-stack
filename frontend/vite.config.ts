import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Dev: the SPA is served on :5190 and proxies /api and /auth to the Rust backend so the
// session cookie is same-origin (the BFF model). Production: the backend (or a CDN) serves
// the built files; there is no Node.js server.
const backend = process.env.APP_BACKEND_URL ?? "http://localhost:8080";

export default defineConfig({
  plugins: [solid()],
  server: {
    port: Number(process.env.DEV_WEB_PORT ?? 5190),
    strictPort: true,
    proxy: {
      "/api": { target: backend, changeOrigin: false, ws: true },
      "/auth": { target: backend, changeOrigin: false },
      "/healthz": backend,
      "/readyz": backend,
      "/version": backend,
    },
  },
  build: {
    target: "es2022",
    sourcemap: "hidden",
    // CSP-friendly output: no inline scripts/styles, hashed assets under /assets.
    assetsInlineLimit: 0,
    cssCodeSplit: true,
    rollupOptions: {
      // Long-lived vendor chunk: framework code changes rarely, so browsers keep it cached.
      output: {
        manualChunks(id: string) {
          if (id.includes("node_modules")) return "vendor";
          return undefined;
        },
      },
    },
  },
  test: {
    environment: "jsdom",
    globals: false,
    include: ["tests/unit/**/*.test.{ts,tsx}"],
    setupFiles: ["tests/unit/setup.ts"],
    server: { deps: { inline: [/solid-js/, /@solidjs\/router/] } },
  },
  resolve: { conditions: ["development", "browser"] },
});
