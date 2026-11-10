// Dronoids' site: a starry backdrop, and the account (see site/dronoids_site.py).
"use strict";

(function () {
    const api = "api/";
    const $ = (id) => document.getElementById(id);
    const COLORS = { google: "#4285f4", discord: "#5865f2", github: "#e6edf3", twitch: "#9146ff", test: "#7dff6a" };
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

    function render(me) {
        const account = me.account;
        $("signed-out").hidden = account !== null;
        $("choose").hidden = !(account && !account.nickname);
        $("signed-in").hidden = !(account && account.nickname);
        const named = account && account.nickname;
        $("nav-account").textContent = named ? account.nickname : "Account";
        $("hero-account").hidden = !!named;
        $("hero-note").hidden = !!named;
        if (!account) {
            providerLinks($("providers"), me.providers, (p) => "Continue with " + p.label);
            $("no-provider").hidden = me.providers.length > 0;
        } else if (!account.nickname) {
            const g = guest();
            if (g && !$("nickname").value) {
                $("nickname").value = g.name;
                $("guest-note").textContent =
                    "It is the nickname you already play under in this browser: your colony comes along.";
                $("guest-note").hidden = false;
            }
            $("nickname").focus();
        } else {
            $("who").textContent = account.nickname;
            const names = Object.fromEntries(me.providers.map((p) => [p.id, p.label]));
            $("linked").textContent = account.linked.map((id) => names[id] || id).join(", ");
            const more = me.providers.filter((p) => !account.linked.includes(p.id));
            providerLinks($("more-providers"), more, (p) => "Link " + p.label);
        }
    }

    async function refresh() {
        try {
            const r = await fetch(api + "me", { credentials: "same-origin" });
            render(await r.json());
        } catch (e) {
            render({ account: null, providers: [] });
            showError("Accounts are unavailable for the moment. The game itself stays open.");
        }
    }

    $("choose").addEventListener("submit", async (e) => {
        e.preventDefault();
        const nickname = $("nickname").value;
        const g = guest();
        try {
            render(await post("nickname", { nickname, guest_token: g && g.name === nickname.trim() ? g.token : null }));
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
