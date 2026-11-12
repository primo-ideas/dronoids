// Dronoids' site: a starry backdrop, and the account (see site/dronoids_site.py):
// a nickname and a password, which the game needs to enter the sector.
"use strict";

(function () {
    const api = "api/";
    const $ = (id) => document.getElementById(id);
    const COLORS = { google: "#4285f4", discord: "#5865f2", github: "#e6edf3", twitch: "#9146ff", test: "#7dff6a" };
    // Sent here by the game: back to it once signed in.
    const fromGame = new URLSearchParams(location.search).get("from") === "play";
    const ERRORS = {
        session: "The sign-in expired on the way: start again.",
        refused: "Sign-in cancelled.",
        provider: "The sign-in service did not answer as expected: try again.",
        "already-linked": "This account is already linked to another Dronoids player.",
    };

    // ----- starry backdrop ---------------------------------------------------

    const sky = $("sky");
    const ctx = sky.getContext("2d");
    let stars = [];
    function resize() {
        sky.width = innerWidth;
        sky.height = innerHeight;
        stars = Array.from({ length: Math.round(innerWidth * innerHeight / 3500) }, () => ({
            x: Math.random() * innerWidth,
            y: Math.random() * innerHeight,
            z: Math.random(),
            t: Math.random() * 6,
        }));
    }
    function draw(time) {
        ctx.clearRect(0, 0, sky.width, sky.height);
        const drift = scrollY * 0.25;
        for (const s of stars) {
            const twinkle = 0.55 + 0.45 * Math.sin(time / 900 + s.t);
            ctx.fillStyle = `rgba(150, 190, 255, ${(0.15 + s.z * 0.55) * twinkle})`;
            const y = (s.y - drift * s.z) % sky.height;
            ctx.fillRect(s.x, y < 0 ? y + sky.height : y, 1 + s.z, 1 + s.z);
        }
        if (!matchMedia("(prefers-reduced-motion: reduce)").matches) {
            requestAnimationFrame(draw);
        }
    }
    addEventListener("resize", resize);
    resize();
    requestAnimationFrame(draw);

    // ----- account -----------------------------------------------------------

    // What the game stored for a guest, in this browser (same site).
    function guest() {
        try {
            const s = JSON.parse(localStorage.getItem("dronoids.settings") || "null");
            return s && typeof s.name === "string" ? s : null;
        } catch (e) {
            return null;
        }
    }

    function showError(text) {
        $("error").textContent = text;
        $("error").hidden = !text;
    }

    async function post(path, body) {
        const r = await fetch(api + path, {
            method: "POST",
            credentials: "same-origin",
            headers: { "Content-Type": "application/json", "X-Dronoids": "1" },
            body: JSON.stringify(body || {}),
        });
        const data = await r.json().catch(() => ({ error: "unreadable answer" }));
        if (!r.ok) {
            throw new Error(data.error || "error");
        }
        return data;
    }

    function providerLinks(target, list, label) {
        target.replaceChildren();
        for (const p of list) {
            const a = document.createElement("a");
            a.href = api + "login/" + encodeURIComponent(p.id);
            const dot = document.createElement("span");
            dot.className = "dot";
            dot.style.background = COLORS[p.id] || "#5fd8ff";
            a.append(dot, label(p));
            target.append(a);
        }
    }

    function showTab(which) {
        for (const name of ["signin", "register"]) {
            $(name).hidden = name !== which;
            $("tab-" + name).setAttribute("aria-selected", String(name === which));
        }
        showError("");
    }

    function render(me) {
        const account = me.account;
        $("signed-out").hidden = account !== null;
        $("choose").hidden = !(account && !account.nickname);
        $("signed-in").hidden = !(account && account.nickname);
        const named = account && account.nickname;
        $("nav-account").textContent = named ? account.nickname : "Account";
        $("hero-account").hidden = !!named;
        $("hero-note").hidden = !!named;
        if (named && fromGame) {
            location.href = "play/";
            return;
        }
        if (!account) {
            providerLinks($("providers"), me.providers, (p) => "Continue with " + p.label);
            $("no-provider").hidden = me.providers.length > 0;
            if ($("signin").hidden && $("register").hidden) {
                showTab("signin");
                offerGuestColony();
            }
        } else if (!account.nickname) {
            const g = guest();
            if (g && !$("nickname").value) {
                $("nickname").value = g.name;
                $("choose-note").textContent =
                    "It is the nickname you played under in this browser: your colony comes along.";
                $("choose-note").hidden = false;
            }
            $("nickname").focus();
        } else {
            $("who").textContent = account.nickname;
            const names = Object.fromEntries(me.providers.map((p) => [p.id, p.label]));
            $("linked").textContent = account.linked.map((id) => names[id] || id).join(", ");
            const more = me.providers.filter((p) => !account.linked.includes(p.id));
            providerLinks($("more-providers"), more, (p) => "Link " + p.label);
            $("linked-box").hidden = account.linked.length === 0 && more.length === 0;
            $("current-box").hidden = !account.password;
            $("password-summary").textContent = account.password ? "Change the password" : "Add a password";
        }
    }

    async function refresh() {
        try {
            const r = await fetch(api + "me", { credentials: "same-origin" });
            render(await r.json());
            if (fromGame) {
                $("account").scrollIntoView();
            }
        } catch (e) {
            render({ account: null, providers: [] });
            showError("Accounts are unavailable for the moment: try again in a little while.");
        }
    }

    // Someone who played in this browser as a guest, before accounts, has a
    // colony waiting under that nickname: offer to make the account with it.
    async function offerGuestColony() {
        const g = guest();
        if (!g) {
            return;
        }
        try {
            const r = await post("guest", { nickname: g.name, guest_token: g.token });
            if (r.known && !$("register-nickname").value) {
                $("register-nickname").value = g.name;
                $("guest-note").textContent =
                    "It is the nickname you played under in this browser: your colony comes along.";
                $("guest-note").hidden = false;
                showTab("register");
            }
        } catch (e) {
            /* nothing to offer */
        }
    }

    // The guest token of the nickname played under in this browser, if it is that one.
    function guestToken(nickname) {
        const g = guest();
        return g && g.name === nickname.trim() ? g.token : null;
    }

    $("tab-signin").addEventListener("click", () => showTab("signin"));
    $("tab-register").addEventListener("click", () => showTab("register"));

    $("signin").addEventListener("submit", async (e) => {
        e.preventDefault();
        try {
            render(await post("signin", { nickname: $("signin-nickname").value, password: $("signin-password").value }));
            showError("");
        } catch (err) {
            showError("Not signed in: " + err.message + ".");
        }
        $("signin-password").value = "";
    });
    $("register").addEventListener("submit", async (e) => {
        e.preventDefault();
        const nickname = $("register-nickname").value;
        const password = $("register-password").value;
        if (password !== $("register-again").value) {
            showError("The two passwords differ.");
            return;
        }
        try {
            render(await post("register", { nickname, password, guest_token: guestToken(nickname) }));
            showError("");
        } catch (err) {
            showError("No account made: " + err.message + ".");
        }
    });
    $("password").addEventListener("submit", async (e) => {
        e.preventDefault();
        try {
            render(await post("password", { current: $("password-current").value, password: $("password-new").value }));
            $("password-current").value = "";
            $("password-new").value = "";
            $("password-done").hidden = false;
            showError("");
        } catch (err) {
            showError("Password unchanged: " + err.message + ".");
        }
    });
    $("choose").addEventListener("submit", async (e) => {
        e.preventDefault();
        const nickname = $("nickname").value;
        try {
            render(await post("nickname", { nickname, guest_token: guestToken(nickname) }));
            showError("");
        } catch (err) {
            showError("This nickname cannot be used: " + err.message + ".");
        }
    });
    $("logout").addEventListener("click", async () => render(await post("logout")));
    $("delete").addEventListener("click", async () => {
        if (confirm("Delete your Dronoids account? There is no going back.")) {
            render(await post("delete"));
        }
    });

    const error = new URLSearchParams(location.search).get("error");
    if (error) {
        showError(ERRORS[error] || "The sign-in did not go through.");
        history.replaceState(null, "", location.pathname + "#account");
        $("account").scrollIntoView();
    }
    refresh();
})();
