#!/usr/bin/env python3
"""Offline regressions for intake ordering and source change reporting."""
import importlib.machinery
import importlib.util
import pathlib
import tempfile
import unittest
import sys
sys.dont_write_bytecode = True
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader('watch', str(pathlib.Path(__file__).with_name('nightly-watch')))
spec = importlib.util.spec_from_loader(loader.name, loader)
watch = importlib.util.module_from_spec(spec)
loader.exec_module(watch)


class WatchTests(unittest.TestCase):
    def test_rpc_discovery_handles_multiline_and_deduplicates(self):
        self.assertEqual(watch.rpc_names('Rpc.make(\n "server.a", {}); Rpc.make(\'server.a\', {}); Rpc.make("server.b", {})'), ['server.a', 'server.b'])

    def test_constants_and_unknown_layout(self):
        self.assertEqual(watch.rpc_names('export const WS_METHODS = { a: "server.a" } as const'), ['server.a'])
        with self.assertRaises(ValueError):
            watch.rpc_names('new unrecognized registry')

    def test_report_preserves_backlog_and_commit_evidence(self):
        data = {'portStatus': {'done': 1, 'skip': 2}, 'releases': [{'tag': 'nightly-test', 'sha': 'a' * 40, 'publishedAt': '2026-11-02', 'url': 'https://github.com/pingdotgg/t3code/releases/test', 'previous': 'old', 'changedFilesSincePinned': ['M\tapps/server/a.ts'], 'rpcAddedSincePinned': ['new.rpc'], 'rpcRemovedSincePinned': ['old.rpc'], 'changesSincePrevious': ['b' * 40 + ' fix native stop']}]}
        text = watch.render(data)
        for expected in ['skip: 2', 'new.rpc', 'old.rpc', 'fix native stop', 'ninguna nightly está certificada']:
            self.assertIn(expected, text)

    def test_network_errors_propagate(self):
        with patch.object(watch.urllib.request, 'urlopen', side_effect=OSError('offline')):
            with self.assertRaises(OSError):
                watch.releases()

    def test_collect_tracks_added_removed_and_intervals(self):
        rows = [{'tag_name': t, 'published_at': 'now', 'html_url': 'https://example.test'} for t in ['new', 'old']]
        def fake_git(cache, *args):
            if args[0] == 'rev-parse':
                return args[1].split('^')[0]
            if args[0] == 'show':
                return 'Rpc.make("baseline", {})' if args[1].startswith(watch.PIN) else 'Rpc.make("added", {})'
            if args[0] == 'diff':
                return 'M\tapps/server/test.ts'
            return 'sha fix'
        with tempfile.TemporaryDirectory(dir='/var/tmp') as directory, patch.object(watch, 'git', side_effect=fake_git), patch.object(watch.subprocess, 'run'):
            result = watch.collect(pathlib.Path(directory), rows)
        self.assertEqual(result[0]['previous'], 'old')
        self.assertEqual(result[1]['previous'], watch.PIN)
        self.assertEqual(result[0]['rpcAddedSincePinned'], ['added'])
        self.assertEqual(result[0]['rpcRemovedSincePinned'], ['baseline'])


if __name__ == '__main__':
    unittest.main()
