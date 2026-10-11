#!/bin/sh
# A tiny web app. The agent must start it and keep it running.
cat > app.py <<'PY'
import http.server, os
PORT = int(os.environ.get("PORT", "8765"))
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"hello from the eval app\n"
        self.send_response(200); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a):
        print("request", self.path, flush=True)
print(f"Listening on http://127.0.0.1:{PORT}", flush=True)
http.server.HTTPServer(("127.0.0.1", PORT), H).serve_forever()
PY
