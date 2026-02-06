#!/usr/bin/env python3
"""Simple test HTTP server that mimics an OpenAI-style API."""
from http.server import HTTPServer, BaseHTTPRequestHandler
import json

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/models' or self.path == '/':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            response = {
                "object": "list",
                "data": [
                    {"id": "test-model", "object": "model", "owned_by": "test"}
                ]
            }
            self.wfile.write(json.dumps(response).encode())
        else:
            self.send_response(404)
            self.send_header('Content-Type', 'text/plain')
            self.end_headers()
            self.wfile.write(b'Not Found')

    def do_POST(self):
        content_length = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(content_length) if content_length > 0 else b''
        
        if self.path == '/chat/completions':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            response = {
                "id": "chatcmpl-test",
                "object": "chat.completion",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "Hello from tunnel!"},
                    "finish_reason": "stop"
                }]
            }
            self.wfile.write(json.dumps(response).encode())
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, format, *args):
        print(f"[upstream] {args[0]}")

if __name__ == '__main__':
    server = HTTPServer(('127.0.0.1', 3001), Handler)
    print('[upstream] listening on http://127.0.0.1:3001')
    server.serve_forever()
