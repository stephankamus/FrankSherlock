import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { execSync } from "child_process";

function gitVersion(): string {
  try {
    return execSync("git describe --tags --always", { encoding: "utf-8" }).trim();
  } catch {
    return "dev";
  }
}

/**
 * Port for the Vite dev server.
 *
 * `tauri dev` reads the port from `build.devUrl`, and the launcher
 * (`scripts/dev-profile.sh`) starts a second profile on another port. Keeping the
 * default here means a plain `npm run dev` still uses 1420.
 */
function devPort(): number {
  const raw = Number(process.env.SHERLOCK_DEV_PORT);
  return Number.isInteger(raw) && raw > 0 && raw < 65536 ? raw : 1420;
}

export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(gitVersion()),
  },
  server: {
    port: devPort(),
    strictPort: true,
    watch: {
      // Vite only serves the React frontend; Tauri watches the Rust side on its own.
      // Without this, every dev server registers a watcher on the whole
      // `src-tauri/target` tree (tens of thousands of Cargo artifacts), and a second
      // profile dies with `ENOSPC: System limit for number of file watchers reached`.
      ignored: ["**/src-tauri/**", "**/dist/**", "**/target/**"]
    }
  },
  clearScreen: false
});
