import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright config for DOM-level tests of the example app's frontend.
 *
 * The tests don't run a real Tauri runtime — they install a mock
 * `window.__TAURI__` via `page.addInitScript` so the inline JS in
 * index.html can be exercised from a plain Chromium page. This proves the
 * example's invoke/listen plumbing produces the right DOM updates given a
 * known IPC event sequence.
 *
 * For a real-Tauri end-to-end test, see follow-ups in docs/SPIKE_NOTES.md
 * (would need tauri-driver + xvfb + Python with buckaroo installed).
 */
export default defineConfig({
    testDir: "./tests",
    fullyParallel: true,
    forbidOnly: !!process.env.CI,
    retries: process.env.CI ? 1 : 0,
    workers: process.env.CI ? 1 : undefined,
    reporter: process.env.CI ? "line" : "list",
    use: {
        baseURL: "http://127.0.0.1:5876",
        trace: "on-first-retry",
    },
    projects: [
        {
            name: "chromium",
            use: { ...devices["Desktop Chrome"] },
        },
    ],
    webServer: {
        command: "pnpm exec serve src --listen 5876 --no-clipboard",
        url: "http://127.0.0.1:5876/",
        reuseExistingServer: !process.env.CI,
        timeout: 30_000,
    },
});
