#!/usr/bin/env python3
"""Private process fixture: retain a real server parent until the test closes it."""
import json
import pathlib
import subprocess
import sys
import time

binary, root = sys.argv[1:]
root = pathlib.Path(root)
home = root / "home"
envelope = dict(mode="desktop", noBrowser=True, port=3773, host="127.0.0.1",
                desktopBootstrapToken="private-parent-seed", tailscaleServeEnabled=False,
                tailscaleServePort=443, t3Home=str(home))
child = subprocess.Popen([binary, "serve", "--no-bridge", "--port", "0", "--bootstrap-fd", "0"],
                         stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
child.stdin.write((json.dumps(envelope) + "\n").encode())
child.stdin.close()
deadline = time.monotonic() + 8
while not (home / "userdata/server-runtime.json").is_file():
    if child.poll() is not None:
        sys.exit("private server exited before readiness")
    if time.monotonic() >= deadline:
        child.kill()
        child.wait()
        sys.exit("private server did not become ready")
    time.sleep(0.02)
(root / "child-pid").write_text(str(child.pid))
# The test terminates this directly owned launcher after opening a pidfd for its child.
while True:
    time.sleep(60)
