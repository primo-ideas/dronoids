"""Tests of the account service: python3 site/test_site.py"""

import http.cookiejar
import json
import os
import shutil
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dronoids_site as ds  # noqa: E402


class FakeProvider(BaseHTTPRequestHandler):
    """Answers like an OAuth provider: code "ok-<id>" is the person <id>."""

    def log_message(self, *args):
        pass

    def do_POST(self):
        form = urllib.parse.parse_qs(self.rfile.read(int(self.headers["Content-Length"])).decode())
        code = form["code"][0]
        body = {"access_token": "tok-" + code[3:]} if code.startswith("ok-") else {"error": "bad_code"}
        self.answer(body)

    def do_GET(self):
        token = self.headers.get("Authorization", "")[len("Bearer tok-") :]
        self.answer({"sub": token, "email": "jamais@garde.example"})

    def answer(self, body):
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


class SiteTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        with open(os.path.join(self.dir, "secret"), "w") as f:
            f.write("secret-de-test-assez-long")
        self.names = os.path.join(self.dir, "primo.accounts.json")
        with open(self.names, "w") as f:
            json.dump({"Ana": "jeton-invite-ana", "Zoe": ds.OWNED}, f)
        self.provider = ThreadingHTTPServer(("127.0.0.1", 0), FakeProvider)
        threading.Thread(target=self.provider.serve_forever, daemon=True).start()
        fake = f"http://127.0.0.1:{self.provider.server_address[1]}"
        self.server = ds.serve(
            {
                "listen": "127.0.0.1:0",
                "base_url": "http://localhost:1/dronoids",
                "database": os.path.join(self.dir, "site.db"),
                "secret_file": os.path.join(self.dir, "secret"),
                "game_accounts": self.names,
                "dev_login": True,
                "providers": {
                    "google": {"client_id": "id", "client_secret": "s", "authorize": fake + "/auth", "token": fake + "/token", "user": fake + "/user"},
                    "discord": {"client_id": "", "client_secret": ""},
                },
            }
        )
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.root = f"http://127.0.0.1:{self.server.server_address[1]}/dronoids/api/"
        self.jar = http.cookiejar.CookieJar()

    def tearDown(self):
        self.server.shutdown()
        self.provider.shutdown()
        self.server.server_close()
        self.server.RequestHandlerClass.site.db.close()
        shutil.rmtree(self.dir, ignore_errors=True)

    def call(self, path, body=None, headers=None, jar=None):
        """(status, json or Location) of a request, with this visitor's cookies."""
        opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar if jar is None else jar), NoRedirect)
        data = None if body is None else json.dumps(body).encode()
        h = {"X-Dronoids": "1"} if body is not None else {}
        h.update(headers or {})
        req = urllib.request.Request(self.root + path, data=data, headers=h)
        try:
            with opener.open(req) as r:
                return r.status, json.loads(r.read())
        except urllib.error.HTTPError as e:
            if e.code == 303:
                return 303, e.headers["Location"]
            return e.code, json.loads(e.read())

    def sign_in_with_google(self, person, jar=None):
        jar = self.jar if jar is None else jar
        status, where = self.call("login/google", jar=jar)
        self.assertEqual(status, 303)
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(where).query)["state"][0]
        return self.call(f"callback/google?code=ok-{person}&state={state}", jar=jar)

    # ------------------------------------------------------------------------

    def test_ticket_is_the_one_the_world_server_expects(self):
        # Same as `the_account_service_signs_alike` in src/ticket.rs.
        self.assertEqual(
            ds.sign_ticket(b"secret-de-test", "Primo", 2_000_000_000),
            "site.2000000000.899a2517086a306754d5318670bb258950402f869c7c99fd89737247d15ec10c",
        )

    def test_pseudos(self):
        self.assertEqual(ds.clean_pseudo("  Le   Pilote_7 ")[0], "Le Pilote_7")
        for bad in ["ab", "x" * 21, "IA Vega", "a<script>", None, 42]:
            self.assertIsNone(ds.clean_pseudo(bad)[0], bad)

    def test_only_configured_providers_are_offered(self):
        status, me = self.call("me")
        self.assertEqual(status, 200)
        self.assertIsNone(me["account"])
        self.assertEqual([p["id"] for p in me["providers"]], ["google", ds.DEV])
        self.assertEqual(self.call("login/discord")[0], 404)

    def test_a_google_sign_in_makes_an_account_then_finds_it_again(self):
        status, where = self.sign_in_with_google("p1")
        self.assertEqual((status, where), (303, "http://localhost:1/dronoids/#compte"))
        me = self.call("me")[1]["account"]
        self.assertEqual((me["pseudo"], me["linked"], me["ticket"]), (None, ["google"], None))
        self.assertEqual(self.call("pseudo", {"pseudo": "Nova"})[0], 200)
        me = self.call("me")[1]["account"]
        self.assertEqual(me["pseudo"], "Nova")
        self.assertTrue(me["ticket"].startswith("site."))
        # Another browser, same Google account: same account.
        other = http.cookiejar.CookieJar()
        self.sign_in_with_google("p1", other)
        self.assertEqual(self.call("me", jar=other)[1]["account"]["pseudo"], "Nova")
        # Nothing but the provider's id is kept.
        db = self.server.RequestHandlerClass.site.db
        self.assertNotIn("garde.example", json.dumps(db.execute("SELECT * FROM identities").fetchall()))

    def test_sign_in_needs_the_state_it_started_with(self):
        self.call("login/google")
        self.assertEqual(self.call("callback/google?code=ok-x&state=forged"), (303, "http://localhost:1/dronoids/?erreur=session#compte"))
        self.assertIsNone(self.call("me")[1]["account"])
        status, where = self.call("login/google")
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(where).query)["state"][0]
        self.assertEqual(self.call(f"callback/google?state={state}")[1], "http://localhost:1/dronoids/?erreur=refuse#compte")
        self.call("login/google")
        self.assertEqual(self.sign_in_with_google("x")[0], 303)
        self.assertIsNotNone(self.call("me")[1]["account"])

    def test_a_bad_code_signs_nobody_in(self):
        status, where = self.call("login/google")
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(where).query)["state"][0]
        self.assertEqual(self.call(f"callback/google?code=nope&state={state}")[1], "http://localhost:1/dronoids/?erreur=fournisseur#compte")
        self.assertIsNone(self.call("me")[1]["account"])

    def test_names_taken_in_the_game_need_their_guest_token(self):
        self.call(f"login/{ds.DEV}?qui=a")
        self.assertEqual(self.call("pseudo", {"pseudo": "ana"})[0], 409)
        self.assertEqual(self.call("pseudo", {"pseudo": "Ana", "guest_token": "autre"})[0], 409)
        self.assertEqual(self.call("pseudo", {"pseudo": "Zoe", "guest_token": ds.OWNED})[0], 409)
        self.assertEqual(self.call("pseudo", {"pseudo": "Ana", "guest_token": "jeton-invite-ana"})[0], 200)
        # Once chosen, it stays; and nobody else can take it, whatever the case.
        self.assertEqual(self.call("pseudo", {"pseudo": "Autre"})[0], 409)
        other = http.cookiejar.CookieJar()
        self.call(f"login/{ds.DEV}?qui=b", jar=other)
        self.assertEqual(self.call("pseudo", {"pseudo": "ANA"}, jar=other)[0], 409)

    def test_linking_a_second_provider_to_the_same_account(self):
        self.call(f"login/{ds.DEV}?qui=a")
        self.call("pseudo", {"pseudo": "Liee"})
        self.sign_in_with_google("g1")
        self.assertEqual(self.call("me")[1]["account"]["linked"], [ds.DEV, "google"])
        # That Google account cannot then be linked to someone else.
        other = http.cookiejar.CookieJar()
        self.call(f"login/{ds.DEV}?qui=b", jar=other)
        self.assertEqual(self.sign_in_with_google("g1", other)[1], "http://localhost:1/dronoids/?erreur=deja-lie#compte")

    def test_writes_need_the_site_header_and_origin(self):
        self.call(f"login/{ds.DEV}?qui=a")
        opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar))
        plain = urllib.request.Request(self.root + "pseudo", data=b'{"pseudo":"Piege"}', headers={"Content-Type": "application/json"})
        with self.assertRaises(urllib.error.HTTPError) as e:
            opener.open(plain)
        self.assertEqual(e.exception.code, 403)
        self.assertEqual(self.call("pseudo", {"pseudo": "Piege"}, {"Origin": "https://ailleurs.example"})[0], 403)
        self.assertIsNone(self.call("me")[1]["account"]["pseudo"])

    def test_logout_and_delete(self):
        self.call(f"login/{ds.DEV}?qui=a")
        self.call("pseudo", {"pseudo": "Partant"})
        self.call("logout", {})
        self.assertIsNone(self.call("me")[1]["account"])
        self.call(f"login/{ds.DEV}?qui=a")
        self.assertEqual(self.call("me")[1]["account"]["pseudo"], "Partant")
        self.call("delete", {})
        self.assertIsNone(self.call("me")[1]["account"])
        self.call(f"login/{ds.DEV}?qui=a")
        self.assertIsNone(self.call("me")[1]["account"]["pseudo"], "a fresh account")
        self.assertEqual(self.call("pseudo", {"pseudo": "Partant"})[0], 200, "the pseudo is free again")

    def test_the_test_login_only_exists_on_ones_own_machine(self):
        site = ds.Site(
            {
                "base_url": "https://primo-ideas.xyz/dronoids",
                "database": ":memory:",
                "secret_file": os.path.join(self.dir, "secret"),
                "dev_login": True,
            }
        )
        self.assertFalse(site.dev)
        self.assertEqual(site.offered(), [])


if __name__ == "__main__":
    unittest.main()
