#!/usr/bin/env python3
"""Dronoids' account service: sign-in with Google, Discord, GitHub or Twitch,
one pseudo per account, and the ticket that tells the world server who is
playing.

It only needs Python's standard library. It listens on the loopback, behind
the web server, under `<site>/api/`:

    GET  api/me              who is signed in, the ticket, the providers on offer
    GET  api/login/<p>       off to the provider (or links it to the account)
    GET  api/callback/<p>    back from the provider
    POST api/pseudo          choose the pseudo, once
    POST api/logout
    POST api/delete          delete the account

What is kept: the pseudo, and for each provider the opaque id it gives for
the person. No e-mail address, no name, no access token.

    python3 dronoids_site.py --config config.json

See `docs/deploy.md` for the configuration file.
"""

import argparse
import hashlib
import hmac
import json
import mimetypes
import os
import secrets
import sqlite3
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SESSION_DAYS = 60
TICKET_DAYS = 7
SESSION_COOKIE = "dronoids_session"
STATE_COOKIE = "dronoids_oauth"
# In the world server's list of names: held by an account (see src/ticket.rs).
OWNED = "site"

# Standard OAuth 2 "authorization code" providers. `id` is where the person's
# id is in the answer of the `user` endpoint. Only the least is asked for.
PROVIDERS = {
    "google": {
        "label": "Google",
        "authorize": "https://accounts.google.com/o/oauth2/v2/auth",
        "token": "https://oauth2.googleapis.com/token",
        "user": "https://openidconnect.googleapis.com/v1/userinfo",
        "scope": "openid",
        "id": ["sub"],
    },
    "discord": {
        "label": "Discord",
        "authorize": "https://discord.com/oauth2/authorize",
        "token": "https://discord.com/api/oauth2/token",
        "user": "https://discord.com/api/users/@me",
        "scope": "identify",
        "id": ["id"],
    },
    "github": {
        "label": "GitHub",
        "authorize": "https://github.com/login/oauth/authorize",
        "token": "https://github.com/login/oauth/access_token",
        "user": "https://api.github.com/user",
        "scope": "",
        "id": ["id"],
    },
    "twitch": {
        "label": "Twitch",
        "authorize": "https://id.twitch.tv/oauth2/authorize",
        "token": "https://id.twitch.tv/oauth2/token",
        "user": "https://api.twitch.tv/helix/users",
        "scope": "",
        "id": ["data", 0, "id"],
        "client_id_header": True,
    },
}
# A stand-in provider for trying the site on one's own machine.
DEV = "essai"


def sign_ticket(secret, name, expires):
    """The ticket the world server checks (the same as `ticket::sign`)."""
    mac = hmac.new(secret, f"dronoids-ticket\n{name}\n{expires}".encode(), hashlib.sha256)
    return f"site.{expires}.{mac.hexdigest()}"


def clean_pseudo(raw):
    """The pseudo as it will be kept, or why it cannot be."""
    if not isinstance(raw, str):
        return None, "pseudo manquant"
    name = " ".join(raw.split())
    if not 3 <= len(name) <= 20:
        return None, "entre 3 et 20 caractères"
    if not all(c.isalnum() or c in " _-" for c in name):
        return None, "lettres, chiffres, espaces, - et _ seulement"
    if name.casefold().startswith("ia "):
        return None, "ce nom est réservé aux colonies du jeu"
    return name, None


class Refused(Exception):
    def __init__(self, status, message):
        super().__init__(message)
        self.status, self.message = status, message


class Site:
    """Accounts, sessions and sign-ins; no HTTP in here."""

    def __init__(self, config):
        self.base_url = config["base_url"].rstrip("/")
        base = urllib.parse.urlsplit(self.base_url)
        self.origin = f"{base.scheme}://{base.netloc}"
        self.path = base.path
        self.secure = base.scheme == "https"
        for path in (config["secret_file"], config["database"]):
            if os.path.dirname(path):
                os.makedirs(os.path.dirname(path), exist_ok=True)
        if not os.path.exists(config["secret_file"]):
            # Made once; the world server reads the same file (--site-secret).
            fd = os.open(config["secret_file"], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w") as f:
                f.write(secrets.token_hex(32) + "\n")
        with open(config["secret_file"], encoding="utf-8") as f:
            self.secret = f.read().strip().encode()
        if len(self.secret) < 16:
            raise SystemExit("secret trop court")
        self.game_accounts = config.get("game_accounts")
        self.static = config.get("static")
        self.providers = {}
        for name, given in (config.get("providers") or {}).items():
            if name in PROVIDERS and given.get("client_id") and given.get("client_secret"):
                self.providers[name] = {**PROVIDERS[name], **given}
        # Never on a real site: anyone could sign in as anyone.
        self.dev = bool(config.get("dev_login")) and base.hostname in ("localhost", "127.0.0.1")
        self.lock = threading.Lock()
        self.db = sqlite3.connect(config["database"], check_same_thread=False)
        if os.path.isfile(config["database"]):
            os.chmod(config["database"], 0o600)
        self.db.executescript(
            """
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS accounts (
                id INTEGER PRIMARY KEY,
                pseudo TEXT,
                pseudo_key TEXT UNIQUE,
                created INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS identities (
                provider TEXT NOT NULL,
                subject TEXT NOT NULL,
                account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                PRIMARY KEY (provider, subject)
            );
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                expires INTEGER NOT NULL
            );
            """
        )
        self.db.commit()

    # ----- sessions -----------------------------------------------------------

    @staticmethod
    def _key(token):
        # Only a hash is stored: the database alone opens no session.
        return hashlib.sha256(token.encode()).hexdigest()

    def open_session(self, account):
        token = secrets.token_urlsafe(32)
        now = int(time.time())
        with self.lock:
            self.db.execute("DELETE FROM sessions WHERE expires <= ?", (now,))
            self.db.execute("INSERT INTO sessions VALUES (?, ?, ?)", (self._key(token), account, now + SESSION_DAYS * 86400))
            self.db.commit()
        return token

    def account_of(self, token):
        if not token:
            return None
        with self.lock:
            row = self.db.execute(
                "SELECT account FROM sessions WHERE id = ? AND expires > ?", (self._key(token), int(time.time()))
            ).fetchone()
        return row[0] if row else None

    def close_session(self, token):
        if token:
            with self.lock:
                self.db.execute("DELETE FROM sessions WHERE id = ?", (self._key(token),))
                self.db.commit()

    # ----- accounts -----------------------------------------------------------

    def offered(self):
        out = [{"id": name, "label": p["label"]} for name, p in self.providers.items()]
        if self.dev:
            out.append({"id": DEV, "label": "Compte d'essai"})
        return out

    def describe(self, account):
        """What the pages need to know about who is signed in."""
        if account is None:
            return {"providers": self.offered(), "account": None}
        with self.lock:
            row = self.db.execute("SELECT pseudo FROM accounts WHERE id = ?", (account,)).fetchone()
            linked = [r[0] for r in self.db.execute("SELECT provider FROM identities WHERE account = ? ORDER BY provider", (account,))]
        pseudo = row[0] if row else None
        ticket = sign_ticket(self.secret, pseudo, int(time.time()) + TICKET_DAYS * 86400) if pseudo else None
        return {"providers": self.offered(), "account": {"pseudo": pseudo, "linked": linked, "ticket": ticket}}

    def sign_in(self, provider, subject, current):
        """The account behind an identity: an existing one, the one signed in
        (which the identity then joins), or a new one."""
        with self.lock:
            row = self.db.execute("SELECT account FROM identities WHERE provider = ? AND subject = ?", (provider, subject)).fetchone()
            if row:
                if current is not None and row[0] != current:
                    raise Refused(409, "deja-lie")
                return row[0]
            if current is None:
                current = self.db.execute("INSERT INTO accounts (created) VALUES (?)", (int(time.time()),)).lastrowid
            self.db.execute("INSERT INTO identities VALUES (?, ?, ?)", (provider, subject, current))
            self.db.commit()
            return current

    def guest_names(self):
        """Names already claimed in the game: name -> guest token (or OWNED)."""
        if not self.game_accounts:
            return {}
        try:
            with open(self.game_accounts, encoding="utf-8") as f:
                names = json.load(f)
            return names if isinstance(names, dict) else {}
        except (OSError, ValueError):
            return {}

    def set_pseudo(self, account, raw, guest_token):
        name, why = clean_pseudo(raw)
        if name is None:
            raise Refused(400, why)
        # A name someone plays under as a guest only goes to whoever holds
        # its token: their colony comes along with it.
        for taken, token in self.guest_names().items():
            if taken.casefold() != name.casefold():
                continue
            mine = taken == name and token != OWNED and isinstance(guest_token, str)
            if not (mine and hmac.compare_digest(token.encode(), guest_token.encode())):
                raise Refused(409, "ce pseudo est déjà pris")
        with self.lock:
            row = self.db.execute("SELECT pseudo FROM accounts WHERE id = ?", (account,)).fetchone()
            if row is None:
                raise Refused(401, "session expirée")
            if row[0] is not None:
                raise Refused(409, "le pseudo ne se change pas")
            try:
                self.db.execute("UPDATE accounts SET pseudo = ?, pseudo_key = ? WHERE id = ?", (name, name.casefold(), account))
                self.db.commit()
            except sqlite3.IntegrityError:
                self.db.rollback()
                raise Refused(409, "ce pseudo est déjà pris") from None
        return name

    def delete(self, account):
        with self.lock:
            self.db.execute("DELETE FROM accounts WHERE id = ?", (account,))
            self.db.commit()

    # ----- providers ----------------------------------------------------------

    def redirect_uri(self, provider):
        return f"{self.base_url}/api/callback/{provider}"

    def authorize_url(self, provider, state):
        p = self.providers[provider]
        query = {
            "client_id": p["client_id"],
            "redirect_uri": self.redirect_uri(provider),
            "response_type": "code",
            "state": state,
        }
        if p["scope"]:
            query["scope"] = p["scope"]
        return p["authorize"] + "?" + urllib.parse.urlencode(query)

    def subject(self, provider, code):
        """Exchange the code for the person's id at the provider."""
        p = self.providers[provider]
        form = urllib.parse.urlencode(
            {
                "client_id": p["client_id"],
                "client_secret": p["client_secret"],
                "code": code,
                "grant_type": "authorization_code",
                "redirect_uri": self.redirect_uri(provider),
            }
        ).encode()
        agent = {"Accept": "application/json", "User-Agent": "dronoids-site"}
        token = _fetch(urllib.request.Request(p["token"], data=form, headers=agent)).get("access_token")
        if not isinstance(token, str) or not token:
            raise Refused(502, "fournisseur")
        headers = {**agent, "Authorization": f"Bearer {token}"}
        if p.get("client_id_header"):
            headers["Client-Id"] = p["client_id"]
        found = _fetch(urllib.request.Request(p["user"], headers=headers))
        try:
            for key in p["id"]:
                found = found[key]
        except (KeyError, IndexError, TypeError):
            raise Refused(502, "fournisseur") from None
        if isinstance(found, bool) or not isinstance(found, (str, int)) or found == "":
            raise Refused(502, "fournisseur")
        return str(found)


def _fetch(request):
    try:
        with urllib.request.urlopen(request, timeout=10) as answer:
            data = json.loads(answer.read(1 << 20))
    except (urllib.error.URLError, OSError, ValueError):
        raise Refused(502, "fournisseur") from None
    if not isinstance(data, dict):
        raise Refused(502, "fournisseur")
    return data


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "dronoids-site"
    sys_version = ""
    site = None  # set by `serve`

    # ----- plumbing -----------------------------------------------------------

    def log_message(self, format, *args):
        # Never the query string: it carries sign-in codes.
        sys.stderr.write(f"{self.command} {self.path.split('?')[0]} {args[1] if len(args) > 1 else ''}\n")

    def cookie(self, name):
        for part in self.headers.get("Cookie", "").split(";"):
            key, _, value = part.strip().partition("=")
            if key == name:
                return value
        return None

    def set_cookie(self, name, value, max_age):
        flags = f"Path={self.site.path}/api/; Max-Age={max_age}; HttpOnly; SameSite=Lax"
        return ("Set-Cookie", f"{name}={value}; {flags}" + ("; Secure" if self.site.secure else ""))

    def reply(self, status, body=b"", kind="application/json; charset=utf-8", headers=()):
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        for key, value in headers:
            self.send_header(key, value)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def json(self, status, data, headers=()):
        self.reply(status, json.dumps(data, ensure_ascii=False).encode(), headers=headers)

    def back_home(self, error=None, headers=()):
        where = self.site.base_url + "/" + (f"?erreur={error}" if error else "") + "#compte"
        self.reply(303, headers=[("Location", where), *headers])

    def route(self):
        path = urllib.parse.urlsplit(self.path).path
        if not path.startswith(self.site.path + "/"):
            return None
        return path[len(self.site.path) + 1 :]

    # ----- GET ----------------------------------------------------------------

    def do_GET(self):
        route = self.route()
        try:
            if route == "api/me":
                self.json(200, self.site.describe(self.site.account_of(self.cookie(SESSION_COOKIE))))
            elif route is not None and route.startswith("api/login/"):
                self.login(route[len("api/login/") :])
            elif route is not None and route.startswith("api/callback/"):
                self.callback(route[len("api/callback/") :])
            elif route is not None and self.site.static and not route.startswith("api/"):
                self.file(route)
            else:
                self.json(404, {"error": "introuvable"})
        except Refused as e:
            self.json(e.status, {"error": e.message})

    do_HEAD = do_GET

    def login(self, provider):
        query = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
        if provider == DEV and self.site.dev:
            # No provider at all: whoever asks is "essai:<qui>".
            who = (query.get("qui") or ["moi"])[0][:40]
            self.signed_in(DEV, who)
            return
        if provider not in self.site.providers:
            raise Refused(404, "fournisseur inconnu")
        state = secrets.token_urlsafe(24)
        self.reply(303, headers=[("Location", self.site.authorize_url(provider, state)), self.set_cookie(STATE_COOKIE, state, 600)])

    def callback(self, provider):
        if provider not in self.site.providers:
            raise Refused(404, "fournisseur inconnu")
        query = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
        forget = self.set_cookie(STATE_COOKIE, "", 0)
        state, expected = (query.get("state") or [""])[0], self.cookie(STATE_COOKIE) or ""
        code = (query.get("code") or [""])[0]
        if not expected or not hmac.compare_digest(state.encode(), expected.encode()):
            self.back_home("session", [forget])
        elif not code:
            # The person said no at the provider (or it failed there).
            self.back_home("refuse", [forget])
        else:
            try:
                subject = self.site.subject(provider, code)
            except Refused as e:
                self.back_home(e.message, [forget])
                return
            self.signed_in(provider, subject, [forget])

    def signed_in(self, provider, subject, headers=()):
        current = self.site.account_of(self.cookie(SESSION_COOKIE))
        try:
            account = self.site.sign_in(provider, subject, current)
        except Refused as e:
            self.back_home(e.message, headers)
            return
        if account != current:
            headers = [*headers, self.set_cookie(SESSION_COOKIE, self.site.open_session(account), SESSION_DAYS * 86400)]
        self.back_home(None, headers)

    def file(self, route):
        """Development only: the pages, next to the API (same origin)."""
        root = os.path.realpath(self.site.static)
        path = os.path.realpath(os.path.join(root, route))
        if os.path.isdir(path):
            path = os.path.join(path, "index.html")
        if not (path == root or path.startswith(root + os.sep)) or not os.path.isfile(path):
            raise Refused(404, "introuvable")
        kind = "application/wasm" if path.endswith(".wasm") else mimetypes.guess_type(path)[0] or "application/octet-stream"
        with open(path, "rb") as f:
            self.reply(200, f.read(), kind)

    # ----- POST ---------------------------------------------------------------

    def do_POST(self):
        route = self.route()
        try:
            # Another site cannot send this header (nor read the answer): a
            # page elsewhere cannot act on a visitor's account.
            origin = self.headers.get("Origin")
            if self.headers.get("X-Dronoids") != "1" or (origin is not None and origin != self.site.origin):
                raise Refused(403, "requête refusée")
            length = int(self.headers.get("Content-Length") or 0)
            if not 0 <= length <= 4096:
                raise Refused(413, "requête trop longue")
            try:
                body = json.loads(self.rfile.read(length) or b"{}")
            except ValueError:
                raise Refused(400, "requête illisible") from None
            if not isinstance(body, dict):
                raise Refused(400, "requête illisible")
            token = self.cookie(SESSION_COOKIE)
            account = self.site.account_of(token)
            if route == "api/logout":
                self.site.close_session(token)
                self.json(200, self.site.describe(None), [self.set_cookie(SESSION_COOKIE, "", 0)])
            elif route in ("api/pseudo", "api/delete") and account is None:
                raise Refused(401, "session expirée")
            elif route == "api/pseudo":
                self.site.set_pseudo(account, body.get("pseudo"), body.get("guest_token"))
                self.json(200, self.site.describe(account))
            elif route == "api/delete":
                self.site.delete(account)
                self.json(200, self.site.describe(None), [self.set_cookie(SESSION_COOKIE, "", 0)])
            else:
                raise Refused(404, "introuvable")
        except Refused as e:
            self.json(e.status, {"error": e.message})


def serve(config):
    """The HTTP server for this configuration, ready to `serve_forever`."""
    host, _, port = config.get("listen", "127.0.0.1:8083").rpartition(":")
    handler = type("SiteHandler", (Handler,), {"site": Site(config)})
    server = ThreadingHTTPServer((host or "127.0.0.1", int(port)), handler)
    server.daemon_threads = True
    return server


def main():
    parser = argparse.ArgumentParser(description="Dronoids account service")
    parser.add_argument("--config", required=True)
    args = parser.parse_args()
    with open(args.config, encoding="utf-8") as f:
        config = json.load(f)
    server = serve(config)
    site = server.RequestHandlerClass.site
    names = ", ".join(p["label"] for p in site.offered()) or "aucun (voir docs/deploy.md)"
    print(f"comptes Dronoids sur http://{server.server_address[0]}:{server.server_address[1]}{site.path}/api/ — connexion par : {names}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
