"""Pure admission checks; run inside the private Python fixture container."""
import pathlib
import os
import runpy
import tempfile
from unittest import mock
import subprocess
import unittest
from speech_runtime_admission import require_rootless, require_target

class Admission(unittest.TestCase):
    def test_probe_explicitly_checks_local_rootless_security(self):
        calls=[]
        def probe(command,**options):
            calls.append((command,options));return 'true\n'
        require_rootless(probe)
        self.assertEqual(calls,[(['podman','--remote=false','info','--format','{{.Host.Security.Rootless}}'],{'text':True})])

    def test_rootful_missing_and_malformed_security_are_refused(self):
        for response in ['false\n','','True\n','null',' true false ']:
            with self.subTest(response=response),self.assertRaises(RuntimeError):
                require_rootless(lambda *args,**kwargs:response)

    def test_failed_probe_is_not_reinterpreted_as_rootless(self):
        def failed(*args,**kwargs):raise subprocess.CalledProcessError(125,['podman','info'])
        with self.assertRaises(subprocess.CalledProcessError):require_rootless(failed)

    def test_target_must_be_explicit_normalized_and_absolute(self):
        for value in [None,'','relative-target','/tmp/../shared','/tmp/target\n']:
            with self.subTest(value=value),self.assertRaises(ValueError):
                require_target({} if value is None else {'CARGO_TARGET_DIR':value})
        self.assertEqual(require_target({'CARGO_TARGET_DIR':'/fixture/owned-worktree-target'}),pathlib.Path('/fixture/owned-worktree-target'))

    def test_acquisition_entrypoint_refuses_rootful_before_creating_output(self):
        root=pathlib.Path(tempfile.mkdtemp(prefix='speech-wrapper-'))
        destination=root/'not-created'
        command=str(pathlib.Path(__file__).parent/'acquire-speech-engine')
        with mock.patch.object(__import__('sys'),'argv',[command,'--out',str(destination)]),mock.patch('subprocess.check_output',return_value='false\n'):
            with self.assertRaises(RuntimeError):runpy.run_path(command,run_name='__main__')
        self.assertFalse(destination.exists())

    def test_runtime_entrypoint_requires_target_before_creating_artifacts(self):
        root=pathlib.Path(tempfile.mkdtemp(prefix='speech-wrapper-'));inputs=root/'inputs';inputs.mkdir()
        command=str(pathlib.Path(__file__).parent/'test-real-speech-engine')
        arguments=[command,'--inputs',str(inputs),'--node',str(root/'unused-node'),'--evidence-root',str(root)]
        with mock.patch.object(__import__('sys'),'argv',arguments),mock.patch.dict(os.environ,{},clear=True),mock.patch('subprocess.check_output') as probe:
            with self.assertRaises(ValueError):runpy.run_path(command,run_name='__main__')
            probe.assert_not_called()
        self.assertEqual(list(root.iterdir()),[inputs])

    def test_runtime_entrypoint_refuses_rootful_before_node_admission(self):
        root=pathlib.Path(tempfile.mkdtemp(prefix='speech-wrapper-'));inputs=root/'inputs';inputs.mkdir()
        command=str(pathlib.Path(__file__).parent/'test-real-speech-engine')
        arguments=[command,'--inputs',str(inputs),'--node',str(root/'unused-node'),'--evidence-root',str(root)]
        with mock.patch.object(__import__('sys'),'argv',arguments),mock.patch.dict(os.environ,{'CARGO_TARGET_DIR':str(root/'target')}),mock.patch('subprocess.check_output',return_value='false\n') as probe:
            with self.assertRaises(RuntimeError):runpy.run_path(command,run_name='__main__')
            self.assertEqual(probe.call_count,1)
        self.assertEqual(list(root.iterdir()),[inputs])

if __name__=='__main__':unittest.main()
