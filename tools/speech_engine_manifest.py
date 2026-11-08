"""Private archive-byte producer for the compiled speech engine allowlist."""
import base64
import hashlib
import io
import json
import pathlib
import tarfile
import sys

if not pathlib.Path('/run/.containerenv').is_file():
    raise RuntimeError('Archive inspection requires the private Podman runtime')
source = pathlib.Path('/source')
root = pathlib.Path('/inputs')
pin_bytes = (source / 'runtime/speech/engine-packages-4314.json').read_bytes()
pins = json.loads(pin_bytes)
files = []
seen = set()
for index, pin in enumerate(pins['packages']):
    archive = root / f'package-{index}.tgz'
    import os
    import stat
    with os.fdopen(os.open(archive, os.O_RDONLY | os.O_NOFOLLOW), 'rb') as handle:
        before = os.fstat(handle.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_size > 80 * 1024 * 1024:
            raise RuntimeError('Invalid pinned package archive')
        data = handle.read(80 * 1024 * 1024 + 1)
        after = os.fstat(handle.fileno())
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns):
            raise RuntimeError('Package archive changed during admission')
    if 'sha512-' + base64.b64encode(hashlib.sha512(data).digest()).decode() != pin['integrity']:
        raise RuntimeError('Pinned package archive integrity failed')
    count = size = 0
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as package:
        for member in package:
            parts = pathlib.PurePosixPath(member.name).parts
            if not parts or parts[0] != 'package' or any(part in ('..', '.', '') for part in parts) or '\\' in member.name:
                raise RuntimeError('Invalid archive member name')
            if member.isdir():
                continue
            if not member.isfile() or member.issym() or member.islnk():
                raise RuntimeError('Package member is not an exclusive regular file')
            relative = str(pathlib.PurePosixPath('node_modules', pin['name'], *parts[1:]))
            if relative in seen:
                raise RuntimeError('Duplicate package member')
            seen.add(relative)
            with package.extractfile(member) as contents:
                digest = hashlib.file_digest(contents, 'sha256').hexdigest()
            files.append({'path': relative, 'bytes': member.size, 'sha256': digest})
            count += 1
            size += member.size
    if count != pin['fileCount'] or size != pin['unpackedSize']:
        raise RuntimeError('Incomplete pinned package graph')
files.sort(key=lambda file: file['path'])
if len(files) != 140:
    raise RuntimeError('Unexpected engine graph size')
manifest = {'nativeRevision': pins['nativeRevision'], 'platform': pins['platform'],
            'engineVersion': pins['engineVersion'], 'nodeVersion': pins['nodeVersion'],
            'nodeSha256': '0adde40d1b7dfa57c8bc83cbc319758c3ec6ebba21363bb2fa752ea6b267ad1b',
            'entry': 'node_modules/transcribe-cpp/dist/index.js',
            'packagePinsSha256': hashlib.sha256(pin_bytes).hexdigest(), 'files': files}
json.dump(manifest, sys.stdout, indent=2)
print()
