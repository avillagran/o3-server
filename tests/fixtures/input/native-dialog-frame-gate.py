#!/usr/bin/python3
"""Hold actual frame discovery while forwarding a real owned Chromium dialog."""
import json
import os
import pathlib
import select
import socket
import stat
import subprocess
import sys
import time

assert pathlib.Path('/run/.containerenv').is_file()
gate = pathlib.Path(__file__).parent
identity = gate.stat()
assert identity.st_uid == os.getuid() and stat.S_ISDIR(identity.st_mode)
assert stat.S_IMODE(identity.st_mode) == 0o700

def publish(name, value):
    temporary = gate / (name + '.partial')
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'w') as stream:
        json.dump(value, stream)
        stream.flush()
        os.fsync(stream.fileno())
    os.rename(temporary, gate / name)

parent, child = socket.socketpair()
def descriptors():
    os.dup2(child.fileno(), 3)
    os.dup2(child.fileno(), 4)

process = None
root = None
stderr = None
LIMIT = 4 * 1024 * 1024
held_bytes = 0
try:
    stderr = os.open(gate / 'chromium.stderr', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    process = subprocess.Popen(['/usr/bin/chromium', '--no-sandbox', '--disable-dev-shm-usage', *sys.argv[1:]],
                               pass_fds=(3, 4, child.fileno()), preexec_fn=descriptors, stderr=stderr)
    os.close(stderr)
    stderr = None
    child.close()
    root = socket.socket(fileno=os.dup(3))
    root.settimeout(2)
    parent.settimeout(2)
    buffers = {root: b'', parent: b''}
    # Native pending IDs are positive u64 values; Chromium dispatch uses signed int
    # call IDs. Refuse overlap rather than rewriting any native request/reply ID.
    fixture_id = -1
    session = None
    frame_id = None
    held = []
    held_since = None
    released = False
    while process.poll() is None:
        if held and not released and os.path.lexists(gate / 'release-frame'):
            info = os.lstat(gate / 'release-frame')
            assert stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1
            for packet in held:
                root.sendall(packet)
            publish('frame-released.json', {'sessionId': session, 'requestId': frame_id, 'originalPackets': len(held)})
            held = []
            held_bytes = 0
            released = True
        if held_since is not None and not released and time.monotonic() - held_since > 3:
            raise TimeoutError('Owned frame gate exceeded its bound; no automatic successful release')
        readable, _, _ = select.select([root, parent], [], [], 0.01)
        for incoming in readable:
            data = incoming.recv(65536)
            if not data:
                raise EOFError()
            buffers[incoming] += data
            while b'\0' in buffers[incoming]:
                packet, buffers[incoming] = buffers[incoming].split(b'\0', 1)
                if len(packet) > LIMIT:
                    raise ValueError('Actual CDP packet exceeds native four-MiB admission')
                message = json.loads(packet)
                if incoming is root:
                    assert type(message.get('id')) is int and message['id'] > 0
                    assert message['id'] != fixture_id
                    if message.get('method') == 'Page.enable' and session is None:
                        session = message['sessionId']
                        assert isinstance(session, str) and session
                    if message.get('method') == 'Page.getFrameTree' and frame_id is None:
                        assert message['sessionId'] == session
                        frame_id = message['id']
                    parent.sendall(packet + b'\0')
                    continue
                if message.get('id') == fixture_id:
                    assert message.get('sessionId') == session
                    publish('evaluation-done.json', message)
                    continue
                scoped = session is not None and message.get('sessionId') == session
                if not released and ((frame_id is not None and message.get('id') == frame_id) or (scoped and message.get('method') == 'Page.frameNavigated')):
                    held_bytes += len(packet) + 1
                    if held_bytes > LIMIT or len(held) >= 8:
                        raise ValueError('Held original packets exceed bounded admission')
                    held.append(packet + b'\0')
                    if held_since is None:
                        held_since = time.monotonic()
                    if message.get('id') == frame_id:
                        assert 'result' in message and 'error' not in message
                        publish('frame-held.json', {'sessionId': session, 'requestId': frame_id, 'actualResponse': message})
                        command = {'id': fixture_id, 'sessionId': session, 'method': 'Runtime.evaluate',
                                   'params': {'expression': "alert('owned-before-frame')", 'returnByValue': True}}
                        parent.sendall(json.dumps(command).encode() + b'\0')
                    continue
                if scoped and message.get('method') == 'Page.javascriptDialogOpening':
                    publish('actual-opening.json', message)
                root.sendall(packet + b'\0')
            if len(buffers[incoming]) > LIMIT:
                raise ValueError('Unterminated CDP buffer exceeds native four-MiB admission')
except (EOFError, BrokenPipeError, ConnectionResetError):
    pass
finally:
    if stderr is not None:
        os.close(stderr)
    child.close()
    if process is not None:
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    if root is not None:
        root.close()
    parent.close()
