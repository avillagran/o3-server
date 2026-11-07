"""Pure lexical regression checks; no fixture filesystem operations."""
import unittest
from test_safety import recursive_calls


class SafetyScannerTests(unittest.TestCase):
    def test_test_scopes(self):
        for attribute in ('#[test]', '#[tokio::test]', '#[cfg(test)]'):
            with self.subTest(attribute=attribute):
                self.assertEqual(len(recursive_calls(attribute + ' fn check() { fs::remove_dir_all(p); }')), 1)

    def test_nested_module_and_integration(self):
        self.assertEqual(len(recursive_calls('#[cfg(test)] mod tests { fn helper() { fs::remove_dir_all(p); } }')), 1)
        self.assertEqual(len(recursive_calls('fn helper() { fs::remove_dir_all(p); }', True)), 1)

    def test_production_scope_is_distinct(self):
        self.assertEqual(recursive_calls('fn production() { fs::remove_dir_all(p); }'), [])
        self.assertEqual(recursive_calls('#[cfg(not(test))] fn production() { fs::remove_dir_all(p); }'), [])

    def test_inert_text_and_lifetimes(self):
        source = '''#[test] fn sample<'a>() {
            let text = r###"remove_dir_all { }"###;
            let text = "remove_dir_all \\\" }";
            let character = '}';
            /* nested /* remove_dir_all */ comment */
            // remove_dir_all
            fs::remove_dir_all(p);
        }'''
        self.assertEqual(len(recursive_calls(source)), 1)

    def test_scope_does_not_leak(self):
        source = '#[test] fn safe() {} fn production() { fs::remove_dir_all(p); }'
        self.assertEqual(recursive_calls(source), [])


if __name__ == '__main__':
    unittest.main()
