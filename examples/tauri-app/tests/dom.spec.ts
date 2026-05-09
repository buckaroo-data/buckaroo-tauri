import { test, expect, type Page } from "@playwright/test";

/**
 * DOM-level tests against the example app's frontend.
 *
 * The frontend uses `window.__TAURI__.core.invoke` and `window.__TAURI__.event.listen`.
 * We install a controllable mock before any inline script runs, then drive it
 * from test code by firing fake events and asserting the resulting DOM.
 *
 * What this proves:
 *   - The example's IPC wiring updates the DOM correctly when events fire.
 *   - Status bar transitions from "Waiting" → "Sidecar ready" on sidecar:ready.
 *   - Load button enables when the sidecar reports ready.
 *   - Each `buckaroo:msg` event appends to the message log.
 *   - `initial_state` messages populate the model-state pre block.
 *
 * What this does NOT prove:
 *   - The Rust supervisor really spawns Python.
 *   - The Tauri IPC actually routes invoke/listen between webview and Rust.
 *   - Real binary parquet frames decode correctly.
 *
 * Those are covered by the architectural validation logs (see PLAN.md +
 * SPIKE_NOTES.md) and by `cargo test` once the Rust side grows tests.
 */

const TAURI_MOCK = `
    // Test-controllable mock of @tauri-apps/api/core + /event globals.
    // Tests fire events via window.__test.fire("event-name", payload).
    // Tests inspect invoke calls via window.__test.calls.
    // Tests influence invoke return values via window.__test.invokeReturns.
    window.__test = {
        handlers: new Map(),
        calls: [],
        invokeReturns: {
            "plugin:buckaroo-tauri|buckaroo_health": { error: "not ready" },
        },
        fire(eventName, payload) {
            const set = this.handlers.get(eventName);
            if (!set) return 0;
            for (const h of set) h({ payload, event: eventName, id: 0 });
            return set.size;
        },
        setInvokeResult(cmd, value) {
            this.invokeReturns[cmd] = value;
        },
    };

    window.__TAURI__ = {
        core: {
            invoke: async (cmd, args) => {
                window.__test.calls.push({ cmd, args });
                const result = window.__test.invokeReturns[cmd];
                if (result && result.error) throw new Error(result.error);
                return result;
            },
        },
        event: {
            listen: async (eventName, handler) => {
                if (!window.__test.handlers.has(eventName)) {
                    window.__test.handlers.set(eventName, new Set());
                }
                window.__test.handlers.get(eventName).add(handler);
                return () => window.__test.handlers.get(eventName).delete(handler);
            },
        },
    };
`;

test.beforeEach(async ({ page }) => {
    await page.addInitScript({ content: TAURI_MOCK });
});

async function fireEvent(page: Page, name: string, payload: unknown) {
    await page.evaluate(
        ([name, payload]) => (window as any).__test.fire(name, payload),
        [name, payload] as const,
    );
}

test("status starts in waiting state, button disabled", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("#status")).toContainText("Waiting for sidecar");
    await expect(page.locator("#load-btn")).toBeDisabled();
});

test("sidecar:ready event flips status to ready and enables load button", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("#status")).toContainText("Waiting for sidecar");

    await fireEvent(page, "buckaroo:sidecar_ready", 58291);

    await expect(page.locator("#status")).toContainText("Sidecar ready");
    await expect(page.locator("#status")).toContainText("58291");
    await expect(page.locator("#load-btn")).toBeEnabled();
    await expect(page.locator("#log")).toContainText("sidecar:ready port=58291");
});

test("buckaroo:msg events append to message log", async ({ page }) => {
    await page.goto("/");
    await fireEvent(page, "buckaroo:sidecar_ready", 58291);

    await fireEvent(page, "buckaroo:msg", {
        type: "metadata",
        path: "/data/sample.parquet",
        rows: 1000,
    });

    await expect(page.locator("#msgs")).toContainText("metadata:");
    await expect(page.locator("#msgs")).toContainText("/data/sample.parquet");
});

test("initial_state populates the model-state pre block", async ({ page }) => {
    await page.goto("/");
    await fireEvent(page, "buckaroo:sidecar_ready", 58291);

    await fireEvent(page, "buckaroo:msg", {
        type: "initial_state",
        protocol_version: 1,
        df_meta: { total_rows: 1000, num_columns: 4 },
        df_data_dict: { main: [] },
        mode: "viewer",
    });

    await expect(page.locator("#msgs")).toContainText("initial_state:");
    await expect(page.locator("#state")).toContainText("total_rows");
    await expect(page.locator("#state")).toContainText("1000");
});

test("sidecar:failed shows the error", async ({ page }) => {
    await page.goto("/");
    await fireEvent(page, "buckaroo:sidecar_failed", "could not find python");

    await expect(page.locator("#status")).toContainText("Sidecar failed");
    await expect(page.locator("#status")).toContainText("could not find python");
});

test("load button click triggers buckaroo_load_path invoke", async ({ page }) => {
    await page.goto("/");
    await fireEvent(page, "buckaroo:sidecar_ready", 58291);

    // Mock a successful load response.
    await page.evaluate(() => {
        (window as any).__test.setInvokeResult(
            "plugin:buckaroo-tauri|buckaroo_load_path",
            { sessionId: "abc123", rows: 1000, metadata: {} },
        );
    });

    await page.locator("#path").fill("/data/sample.parquet");
    await page.locator("#load-btn").click();

    await expect(page.locator("#log")).toContainText("buckaroo_load_path path=/data/sample.parquet");
    await expect(page.locator("#log")).toContainText("loaded session=abc123 rows=1000");

    // Verify the Rust IPC was actually called with the expected payload shape.
    const calls = await page.evaluate(() => (window as any).__test.calls);
    const loadCall = calls.find(
        (c: any) => c.cmd === "plugin:buckaroo-tauri|buckaroo_load_path",
    );
    expect(loadCall).toBeTruthy();
    expect(loadCall.args).toEqual({ args: { path: "/data/sample.parquet" } });
});
