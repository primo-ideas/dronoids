// Dronoids: the bits of the browser the game needs, as a miniquad
// plugin. WebSockets (the game's only network), local storage (settings and
// programs), the default server address and a good source of randomness.
"use strict";

(function () {
    const sockets = new Map();
    let nextSocket = 1;
    const encoder = new TextEncoder();

    function readString(ptr, len) {
        return new TextDecoder().decode(new Uint8Array(wasm_memory.buffer, ptr, len));
    }

    function writeBytes(bytes, ptr, cap) {
        const n = Math.min(bytes.length, cap);
        new Uint8Array(wasm_memory.buffer, ptr, n).set(bytes.subarray(0, n));
        return n;
    }

    // Same host and directory as the page: https://site/dronoids/ -> wss://site/dronoids/ws.
    // A page served locally talks to a local server; ?server= overrides both.
    function defaultServer() {
        const params = new URLSearchParams(window.location.search);
        if (params.get("server")) {
            return params.get("server");
        }
        const host = window.location.hostname;
        if (window.location.protocol === "file:" || host === "127.0.0.1" || host === "localhost") {
            return "ws://127.0.0.1:8082/ws";
        }
        const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
        const dir = window.location.pathname.replace(/[^/]*$/, "");
        return scheme + "//" + window.location.host + dir + "ws";
    }

    // The game was called "Game of Drones" before: what it stored then
    // (notably the secret that owns your name) is read back once and moved.
    function storageKey(ptr, len) {
        const name = readString(ptr, len);
        const key = "dronoids." + name;
        try {
            if (window.localStorage.getItem(key) === null) {
                const old = window.localStorage.getItem("game-of-drones." + name);
                if (old !== null) {
                    window.localStorage.setItem(key, old);
                }
            }
        } catch (e) { /* storage unavailable */ }
        return key;
    }

    miniquad_add_plugin({
        name: "god",
        version: 1,
        register_plugin: function (importObject) {
            const env = importObject.env;

            env.god_ws_open = function (ptr, len) {
                const id = nextSocket++;
                const s = { ws: null, inbox: [], state: 0 };
                sockets.set(id, s);
                try {
                    const ws = new WebSocket(readString(ptr, len));
                    ws.binaryType = "arraybuffer";
                    ws.onopen = function () { s.state = 1; };
                    ws.onmessage = function (e) {
                        if (e.data instanceof ArrayBuffer) {
                            s.inbox.push(new Uint8Array(e.data));
                        }
                    };
                    ws.onclose = function () { s.state = 2; };
                    ws.onerror = function () { s.state = 2; };
                    s.ws = ws;
                } catch (e) {
                    console.warn("WebSocket:", e);
                    s.state = 2;
                }
                return id;
            };
            env.god_ws_state = function (id) {
                const s = sockets.get(id);
                return s ? s.state : 2;
            };
            env.god_ws_send = function (id, ptr, len) {
                const s = sockets.get(id);
                if (s && s.state === 1) {
                    s.ws.send(new Uint8Array(wasm_memory.buffer, ptr, len).slice());
                }
            };
            env.god_ws_peek = function (id) {
                const s = sockets.get(id);
                return s && s.inbox.length > 0 ? s.inbox[0].length : -1;
            };
            env.god_ws_take = function (id, ptr, len) {
                const s = sockets.get(id);
                if (s && s.inbox.length > 0) {
                    writeBytes(s.inbox.shift(), ptr, len);
                }
            };
            env.god_ws_close = function (id) {
                const s = sockets.get(id);
                if (s) {
                    try { if (s.ws) { s.ws.close(); } } catch (e) { /* already closed */ }
                    sockets.delete(id);
                }
            };

            env.god_storage_len = function (kptr, klen) {
                try {
                    const v = window.localStorage.getItem(storageKey(kptr, klen));
                    return v === null ? -1 : encoder.encode(v).length;
                } catch (e) {
                    return -1;
                }
            };
            env.god_storage_read = function (kptr, klen, ptr, cap) {
                try {
                    const v = window.localStorage.getItem(storageKey(kptr, klen)) || "";
                    writeBytes(encoder.encode(v), ptr, cap);
                } catch (e) { /* storage unavailable */ }
            };
            env.god_storage_write = function (kptr, klen, vptr, vlen) {
                try {
                    window.localStorage.setItem(storageKey(kptr, klen), readString(vptr, vlen));
                } catch (e) { /* private mode, quota... */ }
            };

            env.god_default_server = function (ptr, cap) {
                return writeBytes(encoder.encode(defaultServer()), ptr, cap);
            };
            env.god_random = function () {
                return window.crypto.getRandomValues(new Uint32Array(1))[0];
            };
        },
        on_init: function () {
            // The game draws from now on: drop the loading text behind the canvas.
            const loading = document.getElementById("loading");
            if (loading) {
                loading.remove();
            }
        },
    });
})();
