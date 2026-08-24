import http.server
import json
import os
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 9091
SCORES = os.environ.get("SCORES_FILE", "/tmp/xdp-lb-scores.json")


def load_scores():
    try:
        with open(SCORES) as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return {}


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if not self.path.startswith("/api/v1/query"):
            self.send_error(404)
            return

        result = [
            {
                "metric": {"instance": f"{instance}:9100", "job": "node"},
                "value": [1700000000.0, str(value)],
            }
            for instance, value in load_scores().items()
        ]
        body = json.dumps(
            {"status": "success", "data": {"resultType": "vector", "result": result}}
        ).encode()

        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
