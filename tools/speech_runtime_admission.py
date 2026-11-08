"""Fail-closed host admission for private speech acquisition and acceptance."""
import os
import pathlib
import subprocess


def require_rootless(probe=None):
    probe = subprocess.check_output if probe is None else probe
    value = probe([
        'podman', '--remote=false', 'info', '--format',
        '{{.Host.Security.Rootless}}',
    ], text=True)
    if value.strip() != 'true':
        raise RuntimeError('Speech acquisition and acceptance require rootless Podman')


def require_target(environment=None):
    environment = os.environ if environment is None else environment
    value = environment.get('CARGO_TARGET_DIR')
    if (not value or not os.path.isabs(value) or os.path.normpath(value) != value
            or any(character in value for character in '\r\n\0')):
        raise ValueError('Supply an explicit absolute CARGO_TARGET_DIR unique to this worktree')
    return pathlib.Path(value)
