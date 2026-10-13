#!/usr/bin/env python3
"""The server `send-local.yaml` sends to: each path answers with one fact about the request it
received, as plain text, so a walkthrough can assert on exactly what went over the wire.

    python3 dayscript/echo-server.py [port]        # default 8765, on 127.0.0.1

    /hello.json        a small JSON document
    /header/<name>     the value of that request header, or "(none)"
    /query             the raw query string
    /users/<id>        "user <id>", then the query string when there is one
    /body              the method and content type, then the body; a multipart body is listed
                       one field a line, a file as "name=@filename (N bytes)"
    /redirect          302 to /hello.json
    /slow              answers after 30 seconds
"""
import sys
import time
from email.parser import BytesParser
from email.policy import HTTP
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit


class Echo(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def answer(self, text, status=200, content_type="text/plain; charset=utf-8", headers=()):
        data = text.encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        for name, value in headers:
            self.send_header(name, value)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(data)

    def body_text(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        content_type = self.headers.get("Content-Type") or "(none)"
        essence = content_type.split(";")[0].strip()
        if essence != "multipart/form-data":
            return f"{self.command} {essence}\n{body.decode(errors='replace')}"
        message = BytesParser(policy=HTTP).parsebytes(
            b"Content-Type: " + content_type.encode() + b"\r\n\r\n" + body
        )
        lines = [f"{self.command} {essence}"]
        for part in message.iter_parts():
            name = part.get_param("name", header="content-disposition")
            payload = part.get_payload(decode=True) or b""
            if part.get_filename():
                lines.append(f"{name}=@{part.get_filename()} ({len(payload)} bytes)")
            else:
                lines.append(f"{name}={payload.decode(errors='replace')}")
        return "\n".join(lines)

    def handle_request(self):
        url = urlsplit(self.path)
        path = url.path
        if path == "/hello.json":
            self.answer('{"hello": "day"}', content_type="application/json")
        elif path.startswith("/header/"):
            self.answer(self.headers.get(path[len("/header/"):]) or "(none)")
        elif path == "/query":
            self.answer(url.query)
        elif path.startswith("/users/"):
            who = "user " + path[len("/users/"):]
            self.answer(f"{who} {url.query}" if url.query else who)
        elif path == "/body":
            self.answer(self.body_text())
        elif path == "/redirect":
            self.answer("", status=302, headers=[("Location", "/hello.json")])
        elif path == "/slow":
            time.sleep(30)
            self.answer("late")
        else:
            self.answer("not found", status=404)

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = do_OPTIONS = handle_request

    def log_message(self, *_):
        pass


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    ThreadingHTTPServer(("127.0.0.1", port), Echo).serve_forever()
