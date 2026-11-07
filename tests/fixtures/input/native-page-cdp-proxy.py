#!/usr/bin/python3
"""Private Chromium pipe fixture: delay one actual attach reply, never synthesize it."""
import heapq
import json
import os
import select
import socket
import subprocess
import sys
import time

assert os.path.isfile('/run/.containerenv')
parent, child = socket.socketpair()

def descriptors():
    os.dup2(child.fileno(), 3)
    os.dup2(child.fileno(), 4)

stderr = os.open('/out/owner-pipe/chromium.stderr', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
process = subprocess.Popen(['/usr/bin/chromium', '--no-sandbox', '--disable-dev-shm-usage', *sys.argv[1:]],
                           pass_fds=(3, 4, child.fileno()), preexec_fn=descriptors, stderr=stderr)
os.close(stderr)
child.close()
root = socket.socket(fileno=os.dup(3))
root.settimeout(2)
parent.settimeout(2)
root_buffer = b''
browser_buffer = b''
attachments = 0
delayed_id = None
delayed = []
sequence = 0
try:
    while process.poll() is None:
        now = time.monotonic()
        while delayed and delayed[0][0] <= now:
            _, _, packet = heapq.heappop(delayed)
            root.sendall(packet)
        wait = max(0, min(0.05, delayed[0][0] - now)) if delayed else 0.05
        readable, _, _ = select.select([root, parent], [], [], wait)
        for incoming in readable:
            payload = incoming.recv(65536)
            if not payload:
                raise EOFError()
            if incoming is root:
                root_buffer += payload
                while b'\0' in root_buffer:
                    packet, root_buffer = root_buffer.split(b'\0', 1)
                    message = json.loads(packet)
                    if message.get('method') == 'Target.attachToTarget':
                        attachments += 1
                        if attachments == 5:
                            delayed_id = message['id']
                    parent.sendall(packet + b'\0')
            else:
                browser_buffer += payload
                while b'\0' in browser_buffer:
                    packet, browser_buffer = browser_buffer.split(b'\0', 1)
                    message = json.loads(packet)
                    if delayed_id is not None and message.get('id') == delayed_id:
                        sequence += 1
                        heapq.heappush(delayed, (time.monotonic() + 0.35, sequence, packet + b'\0'))
                        marker = os.open('/out/owner-pipe/native-attach-delayed.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                        os.write(marker, b'{"actualAttachReplyDelayed":true,"delayMs":350}\n')
                        os.close(marker)
                    else:
                        root.sendall(packet + b'\0')
except (EOFError, BrokenPipeError, ConnectionResetError):
    pass
finally:
    if process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
    parent.close()
    root.close()
