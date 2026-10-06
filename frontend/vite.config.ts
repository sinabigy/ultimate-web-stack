import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import { devPort } from "./scripts/dev-ports.mjs";

// Dev: the SPA is served on DEV_WEB_PORT (infra/dev-ports.env) and proxies /api and /auth to the
// Rust backend, so the session cookie is same-origin (the BFF model). Ports are resolved only for
// the dev server: `vite build` also runs where infra/ is absent (the release image's build stage).
// Production: the backend (or a CDN) serves the built files; there is no Node.js server.
export default defineConfig(({ command }) => {
  const backend =
    command === "serve" ? (process.env.APP_BACKEND_URL ?? `http://localhost:${devPort("DEV_API_PORT")}`) : "";
  return {
    plugins: [solid()],
    server: {
      port: command === "serve" ? devPort("DEV_WEB_PORT") : undefined,
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
  };
});
