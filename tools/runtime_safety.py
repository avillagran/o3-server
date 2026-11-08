"""Private runtime admission and retained fixture directories for server tools."""
from contextlib import contextmanager
import os
from pathlib import Path
import tempfile


def require_private_runtime():
    """Use the same local Podman / verified CI Docker admission as Rust fixtures."""
    podman = Path('/run/.containerenv').is_file()
    github_docker = (
        os.environ.get('CI') == 'true'
        and os.environ.get('GITHUB_ACTIONS') == 'true'
        and Path('/.dockerenv').is_file()
    )
    if not (podman or github_docker):
        raise SystemExit(
            'Runtime probes require private rootless Podman locally or a verified '
            'GitHub Docker job container; host execution is refused.'
        )


@contextmanager
def retained_directory(*, prefix, dir=None):
    """Keep every fixture artifact until its disposable container is destroyed."""
    require_private_runtime()
    directory = tempfile.mkdtemp(prefix=prefix, dir=dir)
    yield directory
