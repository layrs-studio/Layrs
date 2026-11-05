"""Loopback example; expose through your HTTPS reverse proxy. Standard library only."""
import hashlib
import hmac
import json
import os
import sqlite3
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def verify(keys, headers, body, now=None):
    """Bind timestamp, delivery identity and exact payload; reject stale replays."""
    if len(body) > 1024 * 1024:
        return False
    try:
        timestamp = headers["x-layrs-timestamp"]
        delivery = headers["x-layrs-delivery"]
        generation = headers["x-layrs-key-generation"]
        secret = bytes.fromhex(keys[generation])
        if len(secret) != 32 or abs((time.time() if now is None else now) - int(timestamp)) > 300:
            return False
        if not delivery or len(delivery) > 256:
            return False
        message = timestamp.encode("ascii") + b"." + delivery.encode("ascii") + b"." + body
        expected = "v1=" + hmac.new(secret, message, hashlib.sha256).hexdigest()
        return hmac.compare_digest(expected, headers["x-layrs-signature"])
    except (KeyError, ValueError, UnicodeError, TypeError):
        return False


class Receiver(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass  # Do not log headers, secrets or request bodies.

    def do_POST(self):
        try:
            length = int(self.headers.get("content-length", "-1"))
            if self.path != "/webhooks/layrs" or not 0 <= length <= 1024 * 1024:
                self.send_error(400)
                return
            self.connection.settimeout(10)
            body = self.rfile.read(length)
            with open(os.environ["LAYRS_WEBHOOK_KEYS_FILE"], "rb") as source:
                keys = json.loads(source.read(8193))
            headers = {name.lower(): value for name, value in self.headers.items()}
            if not verify(keys, headers, body):
                self.send_error(401)
                return
            event = json.loads(body)
            delivery = headers["x-layrs-delivery"]
            if event.get("deliveryId") != delivery or event.get("schemaVersion") != "layrs.webhook-event.v1":
                self.send_error(400)
                return
            digest = hashlib.sha256(body).hexdigest()
            with sqlite3.connect(os.environ["LAYRS_WEBHOOK_RECEIPTS_DB"], timeout=10) as db:
                db.execute("PRAGMA synchronous=FULL")
                db.execute("CREATE TABLE IF NOT EXISTS receipts(id TEXT PRIMARY KEY, digest TEXT NOT NULL, body BLOB NOT NULL)")
                db.execute("BEGIN IMMEDIATE")
                previous = db.execute("SELECT digest FROM receipts WHERE id=?", (delivery,)).fetchone()
                if previous and previous[0] != digest:
                    self.send_error(409)
                    return
                # This durable inbox is the handoff to your tool. A separate worker
                # must execute business effects with the same delivery-id deduplication.
                db.execute("INSERT OR IGNORE INTO receipts VALUES(?,?,?)", (delivery, digest, body))
            self.send_response(204)
            self.end_headers()
        except (OSError, ValueError, sqlite3.Error, KeyError):
            self.send_error(503)


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PORT", "8091"))), Receiver).serve_forever()
