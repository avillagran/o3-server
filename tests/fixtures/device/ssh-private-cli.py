#!/usr/bin/python3
"""Private SSH-shaped fixture. It never executes stdin, invokes ssh, or starts a device tool."""
import json
import pathlib
import select
import socket
import socketserver
import sys
import threading

settings_path = pathlib.Path(__file__).with_suffix('.json')
settings = json.loads(settings_path.read_text())
trace = pathlib.Path(settings['trace'])
def record(value):
    with trace.open('a') as output:
        output.write(json.dumps(value) + '\n')
args = sys.argv[1:]
if '-L' not in args:
    text = sys.stdin.read()
    prefix = 'const o3VerifiedRemoteRequest = '
    if not text.startswith(prefix):
        record({'mode': 'command', 'args': args})
        print('fixture command output')
        sys.exit(0)
    request = json.loads(text[len(prefix):text.index(';')])
    record({'mode': request['mode'], 'owner': request['owner'], 'receipt': request.get('receipt'), 'args': args})
    result = {'protocol': 1, 'owner': request['owner'], 'nodePath': '/private/node', 'platforms': [{'platform': 'ios', 'available': True}, {'platform': 'android', 'available': False, 'reason': 'Private fixture'}], 'verifiedInstalled': True}
    if request['mode'] == 'start':
        result.update({'hubPort': settings['remotePort'], 'receipt': settings['receipt'], 'helpers': {'serveSimAxSettings': None, 'serveSimCli': None}})
    elif request['mode'] == 'stop':
        if request['receipt'] != settings['receipt']:
            sys.exit(7)
        result['stopped'] = True
    print(json.dumps(result))
    sys.exit(0)

parts = args[args.index('-L') + 1].split(':')
if parts[0] != '127.0.0.1' or parts[2] != '127.0.0.1':
    raise RuntimeError('Fixture accepts loopback forwards only')
local_port, remote_port = int(parts[1]), int(parts[3])
counter = pathlib.Path(settings['counter'])
count = int(counter.read_text()) if counter.exists() else 0
counter.write_text(str(count + 1))
record({'mode': 'forward', 'attempt': count, 'args': args})
if count < settings.get('failForwards', 0):
    print('Private fixture port already bound', file=sys.stderr)
    sys.exit(1)

class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        upstream = socket.create_connection(('127.0.0.1', remote_port))
        try:
            peers = [self.request, upstream]
            while True:
                readable, _, _ = select.select(peers, [], [], 1)
                for peer in readable:
                    data = peer.recv(65536)
                    if not data:
                        return
                    (upstream if peer is self.request else self.request).sendall(data)
        finally:
            upstream.close()
class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
with Server(('127.0.0.1', local_port), Handler) as server:
    def lifetime():
        sys.stdin.read()
        server.shutdown()
    threading.Thread(target=lifetime, daemon=True).start()
    server.serve_forever(poll_interval=0.02)
