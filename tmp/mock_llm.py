import http.server
import json
import socketserver
import time

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/v1/models':
            response = {"object": "list", "data": [{"id": "test-model", "object": "model"}]}
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(response).encode())
        elif self.path == '/health':
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain')
            self.end_headers()
            self.wfile.write(b'ok')
        else:
            self.send_response(404)
            self.end_headers()
            self.wfile.write(b'not found')

    def do_POST(self):
        content_length = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(content_length) if content_length > 0 else b''
        
        if self.path == '/v1/chat/completions':
            try:
                req = json.loads(body) if body else {}
            except:
                req = {}
            
            stream = req.get('stream', False)
            
            if stream:
                # SSE streaming response
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.send_header('Cache-Control', 'no-cache')
                self.end_headers()
                
                tokens = ['Hello', ' from', ' the', ' tunnel', '!']
                for i, token in enumerate(tokens):
                    chunk = {
                        "id": "chatcmpl-test",
                        "object": "chat.completion.chunk",
                        "choices": [{
                            "index": 0,
                            "delta": {"content": token},
                            "finish_reason": None
                        }]
                    }
                    self.wfile.write(f'data: {json.dumps(chunk)}\n\n'.encode())
                    self.wfile.flush()
                    time.sleep(0.1)
                
                # Final chunk
                final = {
                    "id": "chatcmpl-test",
                    "object": "chat.completion.chunk",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop"
                    }]
                }
                self.wfile.write(f'data: {json.dumps(final)}\n\n'.encode())
                self.wfile.write(b'data: [DONE]\n\n')
                self.wfile.flush()
            else:
                # Non-streaming response
                response = {
                    "id": "chatcmpl-test",
                    "object": "chat.completion",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "Hello from the tunnel!"},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
                }
                body_out = json.dumps(response).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body_out)))
                self.end_headers()
                self.wfile.write(body_out)
        else:
            self.send_response(404)
            self.end_headers()
            self.wfile.write(b'not found')

    def log_message(self, format, *args):
        pass

with socketserver.TCPServer(('', 3001), Handler) as httpd:
    print('Mock LLM server running on :3001')
    httpd.serve_forever()
