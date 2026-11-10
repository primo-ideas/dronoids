// Dronoids' site: a starry backdrop, and the account (see site/dronoids_site.py).
"use strict";

(function () {
    const api = "api/";
    const $ = (id) => document.getElementById(id);
    const COLORS = { google: "#4285f4", discord: "#5865f2", github: "#e6edf3", twitch: "#9146ff", essai: "#7dff6a" };
    const ERRORS = {
        session: "La connexion a expiré en route : recommencez.",
        refuse: "Connexion annulée.",
        fournisseur: "Le service de connexion n'a pas répondu comme prévu : réessayez.",
        "deja-lie": "Ce compte est déjà relié à un autre joueur de Dronoids.",
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
        const data = await r.json().catch(() => ({ error: "réponse illisible" }));
        if (!r.ok) {
            throw new Error(data.error || "erreur");
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
        $("choose").hidden = !(account && !account.pseudo);
        $("signed-in").hidden = !(account && account.pseudo);
        const named = account && account.pseudo;
        $("nav-account").textContent = named ? account.pseudo : "Compte";
        $("hero-account").hidden = !!named;
        $("hero-note").hidden = !!named;
        if (!account) {
            providerLinks($("providers"), me.providers, (p) => "Continuer avec " + p.label);
            $("no-provider").hidden = me.providers.length > 0;
        } else if (!account.pseudo) {
            const g = guest();
            if (g && !$("pseudo").value) {
                $("pseudo").value = g.name;
                $("guest-note").textContent =
                    "C'est le pseudo sous lequel vous jouez déjà dans ce navigateur : votre colonie viendra avec.";
                $("guest-note").hidden = false;
            }
            $("pseudo").focus();
        } else {
            $("who").textContent = account.pseudo;
            const names = Object.fromEntries(me.providers.map((p) => [p.id, p.label]));
            $("linked").textContent = account.linked.map((id) => names[id] || id).join(", ");
            const more = me.providers.filter((p) => !account.linked.includes(p.id));
            providerLinks($("more-providers"), more, (p) => "Relier " + p.label);
        }
    }

    async function refresh() {
        try {
            const r = await fetch(api + "me", { credentials: "same-origin" });
            render(await r.json());
        } catch (e) {
            render({ account: null, providers: [] });
            showError("Les comptes sont momentanément indisponibles. Le jeu, lui, reste ouvert.");
        }
    }

    $("choose").addEventListener("submit", async (e) => {
        e.preventDefault();
        const pseudo = $("pseudo").value;
        const g = guest();
        try {
            render(await post("pseudo", { pseudo, guest_token: g && g.name === pseudo.trim() ? g.token : null }));
            showError("");
        } catch (err) {
            showError("Ce pseudo n'est pas possible : " + err.message + ".");
        }
    });
    $("logout").addEventListener("click", async () => render(await post("logout")));
    $("delete").addEventListener("click", async () => {
        if (confirm("Supprimer votre compte Dronoids ? C'est définitif.")) {
            render(await post("delete"));
        }
    });

    const error = new URLSearchParams(location.search).get("erreur");
    if (error) {
        showError(ERRORS[error] || "La connexion n'a pas abouti.");
        history.replaceState(null, "", location.pathname + "#compte");
        $("compte").scrollIntoView();
    }
    refresh();
})();
