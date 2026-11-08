"""Explicit, no-follow Node runtime receipts shared by private test harnesses."""
import hashlib
import os
import pathlib
import selectors
import stat
import subprocess
import tempfile
import time

VERSION = "26.13.0"
MAX_BINARY_BYTES = 512 * 1024 * 1024
ENV_KEYS = ("O3_ADMITTED_NODE", "O3_ADMITTED_NODE_SHA256", "O3_ADMITTED_NODE_VERSION")


def isolated():
    return pathlib.Path("/run/.containerenv").is_file() or (
        os.environ.get("GITHUB_ACTIONS") == "true"
        and os.environ.get("CI") == "true"
        and pathlib.Path("/.dockerenv").is_file()
    )


def _open(path):
    text = os.fspath(path)
    if (not os.path.isabs(text) or os.path.normpath(text) != text
            or any(c in text for c in "\r\n\0")):
        raise ValueError("Admitted Node requires a normalized absolute path")
    parts = pathlib.PurePath(text).parts[1:]
    if not parts:
        raise ValueError("Admitted Node must be an executable file")
    directory = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        for part in parts[:-1]:
            next_directory = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=directory)
            os.close(directory)
            directory = next_directory
        fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=directory)
    finally:
        os.close(directory)
    info = os.fstat(fd)
    if (not stat.S_ISREG(info.st_mode) or not info.st_mode & 0o111
            or info.st_mode & 0o6000 or not 0 < info.st_size <= MAX_BINARY_BYTES):
        os.close(fd)
        raise ValueError("Admitted Node must be a bounded, unprivileged regular executable")
    return os.fdopen(fd, "rb"), info


def _identity(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_mode)


def _hash(file):
    digest = hashlib.sha256()
    file.seek(0)
    for block in iter(lambda: file.read(1024 * 1024), b""):
        digest.update(block)
    return digest.hexdigest()


def _sha(value):
    if (not isinstance(value, str) or len(value) != 64
            or any(c not in "0123456789abcdef" for c in value)):
        raise ValueError("Invalid admitted Node SHA-256")
    return value


def probe_version(path, expected_sha, retained_capsule=None):
    if not isolated():
        raise RuntimeError("Node execution requires private Podman or genuine GitHub Docker CI")
    home = pathlib.Path(tempfile.mkdtemp(prefix="o3-node-admission-home-", dir="/tmp"))
    home.chmod(0o700)
    source, info = _open(path)
    source.close()
    snapshot({"path":str(path),"sha256":expected_sha,"bytes":info.st_size}, home/"node-bin"/"node")
    process = subprocess.Popen([str(home/"node-bin"/"node"), "--version"], stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               env={"HOME": str(home), "TMPDIR": str(home), "PATH": "", "LC_ALL": "C"})
    output = bytearray()
    deadline = time.monotonic() + 5
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not selector.select(remaining):
                raise RuntimeError("Admitted Node version probe timed out")
            block = os.read(process.stdout.fileno(), 65-len(output))
            if not block:
                break
            output.extend(block)
            if len(output) > 64:
                raise RuntimeError("Admitted Node version response exceeds its bound")
        if process.wait(timeout=max(0.01, deadline-time.monotonic())) != 0:
            raise RuntimeError("Admitted Node version probe failed")
        version = bytes(output).decode("ascii").strip().removeprefix("v")
        if version == VERSION and retained_capsule is not None:
            # The successful probe already owns this immutable snapshot. Expose
            # that same capsule to its trusted acquisition owner, never copy it.
            retained_capsule(home/"node-bin"/"node")
        return version
    finally:
        selector.close()
        process.stdout.close()
        if process.poll() is None:
            process.kill()
        process.wait()
    # The private home is retained; no fixture tree is removed.


def admit(path, expected_sha=None, probe=probe_version):
    path = pathlib.Path(path)
    if expected_sha is not None:
        _sha(expected_sha)
    file, initial = _open(path)
    with file:
        digest = _hash(file)
        if expected_sha is not None and digest != expected_sha:
            raise ValueError("Admitted Node hash mismatch")
        version = probe(path, digest)
        if version != VERSION:
            raise ValueError("Admitted Node must be version " + VERSION)
        if _identity(initial) != _identity(os.fstat(file.fileno())) or _hash(file) != digest:
            raise ValueError("Admitted Node changed during its version probe")
        current, current_info = _open(path)
        with current:
            if _identity(initial) != _identity(current_info) or _hash(current) != digest:
                raise ValueError("Admitted Node path changed during its version probe")
    return {"path": str(path), "version": version, "sha256": digest, "bytes": initial.st_size}


def environment(receipt, path=None):
    if receipt["version"] != VERSION:
        raise ValueError("Unsupported admitted Node receipt")
    _sha(receipt["sha256"])
    selected = os.fspath(path or receipt["path"])
    if not os.path.isabs(selected) or any(c in selected for c in "\r\n\0"):
        raise ValueError("Invalid admitted Node runtime path")
    return dict(zip(ENV_KEYS, (selected, receipt["sha256"], VERSION)))


def snapshot(receipt, destination):
    """Copy only the sealed executable into a new owned immutable artifact entry."""
    expected = _sha(receipt["sha256"])
    if type(receipt["bytes"]) is not int or not 0 < receipt["bytes"] <= MAX_BINARY_BYTES:
        raise ValueError("Invalid admitted Node size")
    with _open(receipt["path"])[0] as source:
        before = os.fstat(source.fileno())
        destination = pathlib.Path(destination)
        destination.parent.mkdir(mode=0o700)
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC
        fd = os.open(destination, flags, 0o500)
        digest = hashlib.sha256()
        size = 0
        with os.fdopen(fd, "wb") as output:
            for block in iter(lambda: source.read(1024*1024), b""):
                output.write(block)
                digest.update(block)
                size += len(block)
            output.flush()
            os.fsync(output.fileno())
            if _identity(before) != _identity(os.fstat(source.fileno())):
                raise ValueError("Admitted Node changed during runtime snapshot")
        if digest.hexdigest() != expected or size != receipt["bytes"]:
            raise ValueError("Admitted Node snapshot does not match its receipt")
        return {"path": "node-bin/node", "sha256": digest.hexdigest(), "bytes": size}
