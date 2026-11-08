"""Admission and retention regressions; fixture trees stay until container teardown."""
import ast
import os
from pathlib import Path
import unittest
from unittest.mock import patch
import runtime_safety


class RuntimeSafetyTests(unittest.TestCase):
    def test_host_is_refused_before_any_fixture_creation(self):
        with patch.object(runtime_safety.Path, 'is_file', return_value=False), patch.object(runtime_safety.tempfile, 'mkdtemp') as create:
            with self.assertRaises(SystemExit), runtime_safety.retained_directory(prefix='never-created-'):
                self.fail('Host fixture was admitted')
            create.assert_not_called()

    def test_ci_needs_both_environment_flags_and_actual_marker(self):
        with patch.object(runtime_safety.Path, 'is_file', lambda value: str(value) == '/.dockerenv'):
            with patch.dict(os.environ, {'CI': 'true', 'GITHUB_ACTIONS': 'false'}):
                with self.assertRaises(SystemExit):
                    runtime_safety.require_private_runtime()
            with patch.dict(os.environ, {'CI': 'true', 'GITHUB_ACTIONS': 'true'}):
                runtime_safety.require_private_runtime()

    def test_normal_and_failed_contexts_keep_all_owned_fixture_contents(self):
        runtime_safety.require_private_runtime()
        with runtime_safety.retained_directory(prefix='retained-normal-') as directory:
            normal = Path(directory)
            (normal / 'kept').write_text('normal')
        self.assertEqual((normal / 'kept').read_text(), 'normal')
        with self.assertRaisesRegex(RuntimeError, 'fixture failure'):
            with runtime_safety.retained_directory(prefix='retained-failed-') as directory:
                failed = Path(directory)
                (failed / 'kept').write_text('failure')
                raise RuntimeError('fixture failure')
        self.assertEqual((failed / 'kept').read_text(), 'failure')

    def test_manual_child_probes_admit_runtime_before_module_effects(self):
        root = Path(__file__).resolve().parent
        for name in ['measure-rss', 'bench-rss', 'bench-shell-scale', 'desktop-bridge-e2e', 'live-chat-e2e', 'measure-bridge-rss']:
            module = ast.parse((root / name).read_text())
            calls = [node for node in module.body if isinstance(node, ast.Expr) and isinstance(node.value, ast.Call)]
            self.assertIsInstance(calls[0].value.func, ast.Name, name)
            self.assertEqual(calls[0].value.func.id, 'require_private_runtime', name)
        for name, entry in [('live-transfer-e2e', 'main'), ('rss_profile.py', 'run')]:
            module = ast.parse((root / name).read_text())
            function = next(node for node in module.body if isinstance(node, ast.FunctionDef) and node.name == entry)
            guard = next(node for node in function.body if isinstance(node, ast.Expr) and isinstance(node.value, ast.Call) and isinstance(node.value.func, ast.Name) and node.value.func.id == 'require_private_runtime')
            children = [node for node in ast.walk(function) if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and isinstance(node.func.value, ast.Name) and node.func.value.id == 'subprocess']
            self.assertTrue(children, name)
            self.assertLess(guard.lineno, min(node.lineno for node in children), name)
        # Deliberately do not execute a benchmark, real CLI, credential copier or live turn.


if __name__ == '__main__':
    unittest.main()
