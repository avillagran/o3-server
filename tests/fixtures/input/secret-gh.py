#!/usr/bin/python3
"""Private synthetic GitHub credential process; no external authentication."""
import json
from pathlib import Path
import sys
root = Path(sys.argv[0]).parent
with (root / 'gh-calls.jsonl').open('a') as calls:
    calls.write(json.dumps(sys.argv[1:]) + '\n')
sys.stdout.write('fixture-cli-token\n')
