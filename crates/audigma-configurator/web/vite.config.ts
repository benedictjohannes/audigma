import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { viteSingleFile } from "vite-plugin-singlefile";

// The audigma server address for the dev proxy (see README of the plan, §6). Fixed at run time:
//   audigma configure web --no-open --bind 127.0.0.1:12301   and   bun run dev (port 12302).
const target = process.env.AUDIGMA_URL ?? "http://127.0.0.1:12301";

export default defineConfig({
  plugins: [react(), tailwindcss(), viteSingleFile()],
  build: { outDir: "dist", emptyOutDir: true },
  server: {
    proxy: {
      // The server only accepts a state-changing request whose Origin equals its own Host, so
      // the proxy rewrites Origin. The cookie `audigma_token_<port>` is host-scoped (not
      // port-scoped), so the browser sends the one set by visiting the server itself to the dev
      // server too; open the server's printed URL once, then use http://127.0.0.1:12302.
      "/api": {
        target,
        changeOrigin: true,
        configure: (proxy) => {
          proxy.on("proxyReq", (req) => {
            if (req.getHeader("origin")) req.setHeader("origin", target);
          });
        },
      },
    },
  },
});
