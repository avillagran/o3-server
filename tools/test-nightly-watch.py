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
        text = watch.render(data, {'b' * 40: {'status': 'ported', 'o3': 'abc1234'}})
        for expected in ['skip: 2', 'new.rpc', 'old.rpc', '[portado] ' + 'b' * 40 + ' fix native stop — abc1234', 'ninguna nightly está certificada']:
            self.assertIn(expected, text)

    def test_client_and_test_only_commits_are_server_neutral(self):
        self.assertTrue(watch.server_neutral(['apps/web/a.tsx', 'apps/server/src/x.test.ts']))
        self.assertFalse(watch.server_neutral(['apps/web/a.tsx', 'apps/server/src/x.ts']))
        self.assertFalse(watch.server_neutral([]))
        self.assertTrue(watch.triage('d' * 40 + ' ui', {}, {'d' * 40}).startswith('[sin efecto en servidor]'))

    def test_untriaged_commits_are_marked_pending(self):
        self.assertTrue(watch.triage('c' * 40 + ' new', {}).startswith('[pendiente]'))

    def test_surfaces_cover_mobile_and_every_package(self):
        self.assertIn('apps', watch.SURFACES)
        self.assertIn('packages', watch.SURFACES)
        self.assertIn(':(exclude)apps/marketing', watch.SURFACES)

    def test_committed_ledger_is_valid(self):
        self.assertTrue(watch.ledger())

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
            if args[0] == 'log' and '--name-only' in args:
                return '\x01sha\napps/web/a.tsx'
            if args[0] == 'diff':
                return 'M\tapps/server/test.ts\nM\tpackages/source-control-github/a.ts\nA\tapps/mobile/b.ts'
            return 'sha fix'
        with tempfile.TemporaryDirectory() as directory, patch.object(watch, 'git', side_effect=fake_git), patch.object(watch.subprocess, 'run'):
            result = watch.collect(pathlib.Path(directory), rows)
        self.assertEqual(result[0]['serverNeutral'], ['sha'])
        self.assertEqual(result[0]['previous'], 'old')
        self.assertEqual(result[1]['previous'], watch.PIN)
        self.assertEqual(result[0]['rpcAddedSincePinned'], ['added'])
        self.assertEqual(result[0]['rpcRemovedSincePinned'], ['baseline'])
        self.assertEqual(result[0]['areasSincePinned'], {'apps/mobile': 1, 'apps/server': 1, 'packages/source-control-github': 1})


if __name__ == '__main__':
    unittest.main()
