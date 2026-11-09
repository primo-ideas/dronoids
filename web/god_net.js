// Dronoids: the bits of the browser the game needs, as a miniquad
// plugin. WebSockets (the game's only network), local storage (settings and
// programs), the default server address, a good source of randomness and
// the phone's on-screen keyboard.
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

    // The game had another name before: what it stored then
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

    // The phone's keyboard. Browsers only bring it up for a focused text
    // field, and only when the focus comes from a touch being handled: the
    // game arms it when a finger lands on one of its text fields, and the
    // field is focused as that finger is lifted (unless it slid away). What
    // is typed is read back by comparing the hidden field with what it held
    // before: U+0008 for each character erased, then what was added.
    const PAD = "  ";
    let keys = null;
    let typed = "";
    let last = PAD;
    let composing = false;
    let armedAt = -1;
    let touchStart = 0;
    let touchFrom = null;
    let fullHeight = 0;
    let shrunk = false;

    function resetKeys() {
        keys.value = PAD;
        keys.setSelectionRange(PAD.length, PAD.length);
        last = PAD;
    }

    function readKeys() {
        const before = Array.from(last);
        const now = Array.from(keys.value);
        let same = 0;
        while (same < before.length && same < now.length && before[same] === now[same]) {
            same++;
        }
        typed += "\b".repeat(before.length - same) + now.slice(same).join("");
        last = keys.value;
        // Words being composed (suggestions, accents) are left alone until done.
        if (!composing) {
            resetKeys();
        }
    }

    // While the keyboard is up, the game fits in what is left of the screen.
    // A keyboard put away without leaving the field (Android's back button)
    // gives the height back: then leave the field, so that the next touch
    // on a text field brings the keyboard up again.
    function fitCanvas() {
        const vv = window.visualViewport;
        const focused = document.activeElement === keys;
        if (focused && vv) {
            if (vv.height < fullHeight - 100) {
                shrunk = true;
            } else if (shrunk) {
                keys.blur();
                return;
            }
        }
        const canvas = document.getElementById("glcanvas");
        const h = focused && vv ? vv.height + "px" : "";
        if (canvas.style.height !== h) {
            canvas.style.height = h;
            window.dispatchEvent(new Event("resize"));
        }
        window.scrollTo(0, 0);
    }

    function setupKeys() {
        keys = document.getElementById("keys");
        const canvas = document.getElementById("glcanvas");
        if (!keys || !canvas) {
            return;
        }
        resetKeys();
        keys.addEventListener("input", readKeys);
        keys.addEventListener("compositionstart", function () { composing = true; });
        keys.addEventListener("compositionend", function () {
            composing = false;
            readKeys();
        });
        keys.addEventListener("focus", function () {
            fullHeight = window.visualViewport ? window.visualViewport.height : 0;
            shrunk = false;
            fitCanvas();
        });
        keys.addEventListener("blur", fitCanvas);
        if (window.visualViewport) {
            window.visualViewport.addEventListener("resize", fitCanvas);
        }
        canvas.addEventListener("touchstart", function (e) {
            if (e.touches.length === 1) {
                const t = e.changedTouches[0];
                touchStart = performance.now();
                touchFrom = [t.clientX, t.clientY];
            }
        }, { passive: true });
        canvas.addEventListener("touchend", function (e) {
            const t = e.changedTouches[0];
            const still = touchFrom !== null && Math.hypot(t.clientX - touchFrom[0], t.clientY - touchFrom[1]) < 12;
            if (armedAt >= touchStart && still && document.activeElement !== keys) {
                keys.focus({ preventScroll: true });
                resetKeys();
            }
            armedAt = -1;
        }, { passive: true });
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

            env.god_keyboard = function (open) {
                if (open) {
                    armedAt = performance.now();
                } else {
                    armedAt = -1;
                    if (keys && document.activeElement === keys) {
                        keys.blur();
                    }
                }
            };
            env.god_typed_len = function () {
                return typed.length === 0 ? 0 : encoder.encode(typed).length;
            };
            env.god_typed_take = function (ptr, len) {
                writeBytes(encoder.encode(typed), ptr, len);
                typed = "";
            };
        },
        on_init: function () {
            // The game draws from now on: drop the loading text behind the canvas.
            const loading = document.getElementById("loading");
            if (loading) {
                loading.remove();
            }
            setupKeys();
        },
    });
})();
