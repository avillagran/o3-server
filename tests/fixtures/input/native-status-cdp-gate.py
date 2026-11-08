#!/usr/bin/python3
"""Private Chromium pipe fixture: hold one actual attach ACK until an owned release."""
import json
import pathlib
import os
import select
import socket
import stat
import subprocess
import sys

assert os.path.isfile('/run/.containerenv')
parent, child = socket.socketpair()

def descriptors():
    os.dup2(child.fileno(), 3)
    os.dup2(child.fileno(), 4)

gate = pathlib.Path(__file__).parent
stderr = os.open(gate / 'chromium.stderr', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
process = subprocess.Popen(['/usr/bin/chromium', '--no-sandbox', '--disable-dev-shm-usage', *sys.argv[1:]],
                           pass_fds=(3, 4, child.fileno()), preexec_fn=descriptors, stderr=stderr)
os.close(stderr)
child.close()
root = socket.socket(fileno=os.dup(3))
root.settimeout(2)
parent.settimeout(2)
root_buffer = b''
browser_buffer = b''
delayed_id = None
held = None
try:
    while process.poll() is None:
        if held is not None and os.path.lexists(gate / 'release-attach'):
            identity = os.lstat(gate / 'release-attach')
            assert stat.S_ISREG(identity.st_mode) and identity.st_uid == os.getuid() and identity.st_nlink == 1
            root.sendall(held)
            held = None
        wait = 0.05
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
                        marker = gate / 'hold-next-attach'
                        if delayed_id is None and os.path.lexists(marker):
                            identity = os.lstat(marker)
                            assert stat.S_ISREG(identity.st_mode) and identity.st_uid == os.getuid() and identity.st_nlink == 1
                            delayed_id = message['id']
                    parent.sendall(packet + b'\0')
            else:
                browser_buffer += payload
                while b'\0' in browser_buffer:
                    packet, browser_buffer = browser_buffer.split(b'\0', 1)
                    message = json.loads(packet)
                    if delayed_id is not None and message.get('id') == delayed_id:
                        assert held is None
                        held = packet + b'\0'
                        marker = os.open(gate / 'actual-attach-held.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                        os.write(marker, b'{"actualAttachReplyHeld":true}\n')
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
