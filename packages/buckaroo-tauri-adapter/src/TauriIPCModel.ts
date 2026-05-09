/**
 * TauriIPCModel — IModel implementation backed by the buckaroo-tauri Rust plugin.
 *
 * The webview never opens a WebSocket. It talks to the Rust supervisor via
 * Tauri IPC; Rust talks to a user-supplied Python sidecar over an internal
 * localhost WebSocket. This eliminates CSP / cross-origin / firewall concerns.
 *
 * Dependencies expected on the Rust side: register the `buckaroo-tauri` plugin
 * (which exposes `buckaroo_send`, `buckaroo_load_path`, etc.) and the
 * `tauri-plugin-shell` plugin (which the Rust plugin uses to spawn Python).
 */
import type { IModel } from "buckaroo-js-core";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export class TauriIPCModel implements IModel {
    private state: Record<string, any>;
    private pendingChanges: Set<string> = new Set();
    private handlers: Map<string, Set<Function>> = new Map();
    private unlisten: Promise<UnlistenFn>;

    /**
     * @param initialState State the caller has already received via the
     *   buckaroo:msg event channel (or via a synchronous HTTP fetch). Subsequent
     *   server pushes update state in place.
     */
    constructor(initialState: Record<string, any> = {}) {
        this.state = { ...initialState };

        this.unlisten = listen<any>("buckaroo:msg", (event) => {
            const msg = event.payload;
            if (!msg || typeof msg !== "object") return;

            if (msg.type === "initial_state") {
                for (const [k, v] of Object.entries(msg)) {
                    if (k === "type" || k === "protocol_version") continue;
                    this.state[k] = v;
                    this.emit(`change:${k}`, v);
                }
                if ((msg as any).metadata) {
                    this.emit("metadata", (msg as any).metadata, (msg as any).prompt);
                }
                return;
            }

            if (msg.type === "metadata") {
                this.state._metadata = msg;
                this.emit("metadata", msg);
                return;
            }

            // The Rust supervisor pairs the WS protocol's `infinite_resp` text
            // frame with the following binary parquet frame and ships them as
            // one event with `data_b64` injected. Decode it and emit
            // "msg:custom" with [DataView] buffers — matches WebSocketModel's
            // contract on the standalone path so getKeySmartRowCache and
            // friends work identically.
            if (msg.type === "infinite_resp" && typeof msg.data_b64 === "string") {
                const bin = atob(msg.data_b64);
                const bytes = new Uint8Array(bin.length);
                for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
                const buffers = [new DataView(bytes.buffer)];
                // Strip the b64 from the outgoing msg to keep listener payloads small
                const { data_b64: _drop, ...msgClean } = msg as any;
                this.emit("msg:custom", msgClean, buffers);
                return;
            }
        });
    }

    get(key: string): any {
        return this.state[key];
    }

    set(key: string, value: any): void {
        this.state[key] = value;
        this.pendingChanges.add(key);
        this.emit(`change:${key}`, value);
    }

    save_changes(): void {
        if (this.pendingChanges.has("buckaroo_state")) {
            void invoke("plugin:buckaroo-tauri|buckaroo_send", {
                msg: {
                    type: "buckaroo_state_change",
                    new_state: this.state.buckaroo_state,
                },
            });
        }
        this.pendingChanges.clear();
    }

    send(msg: unknown): void {
        void invoke("plugin:buckaroo-tauri|buckaroo_send", { msg });
    }

    on(event: string, handler: (...args: any[]) => void): void {
        if (!this.handlers.has(event)) {
            this.handlers.set(event, new Set());
        }
        this.handlers.get(event)!.add(handler);
    }

    off(event: string, handler: (...args: any[]) => void): void {
        this.handlers.get(event)?.delete(handler);
    }

    /** Detach the buckaroo:msg listener. Call on unmount. */
    async dispose(): Promise<void> {
        const unlistenFn = await this.unlisten;
        unlistenFn();
    }

    private emit(event: string, ...args: any[]): void {
        const set = this.handlers.get(event);
        if (set) {
            for (const h of set) {
                try {
                    h(...args);
                } catch (e) {
                    console.error(`[TauriIPCModel] handler error for ${event}:`, e);
                }
            }
        }
    }
}

/**
 * Wait for the first `initial_state` message from the buckaroo server.
 *
 * Use this in a host app's mount sequence:
 *
 * ```ts
 * const initial = await waitForInitialState();
 * const model = new TauriIPCModel(initial);
 * ```
 */
export async function waitForInitialState(): Promise<Record<string, any>> {
    return new Promise((resolve) => {
        const unlistenP = listen<any>("buckaroo:msg", (event) => {
            const msg = event.payload;
            if (msg?.type === "initial_state") {
                unlistenP.then((fn) => fn());
                resolve(msg);
            }
        });
    });
}

/**
 * Convenience: trigger the Rust plugin's load_path command. Returns the
 * sessionId minted by the buckaroo server.
 */
export async function loadPath(path: string): Promise<{ sessionId: string; rows?: number; metadata: any }> {
    return invoke("plugin:buckaroo-tauri|buckaroo_load_path", { args: { path } });
}
