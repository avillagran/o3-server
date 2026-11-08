#!/usr/bin/env python3
"""Private synthetic receipt and immutable executable snapshot regressions."""
import hashlib
import json
import os
import pathlib
import tempfile
import unittest
from unittest import mock
import node_admission as admission


class Admission(unittest.TestCase):
    def setUp(self):
        if not admission.isolated():
            raise RuntimeError("Admission tests require private Podman or genuine GitHub Docker CI")
        self.root=pathlib.Path(tempfile.mkdtemp(prefix="o3-node-admission-case-",dir="/tmp"))
        self.root.chmod(0o700)
        self.node=self.root/"node"
        self.program("print('v26.13.0')")

    def program(self, body):
        self.node.write_text("#!/usr/bin/python3\n"+body+"\n")
        self.node.chmod(0o700)
        return hashlib.sha256(self.node.read_bytes()).hexdigest()

    def test_successful_probe_exposes_its_existing_capsule_without_another_snapshot(self):
        retained=[]
        digest=hashlib.sha256(self.node.read_bytes()).hexdigest()
        with mock.patch.object(admission,"snapshot",wraps=admission.snapshot) as snapshots:
            receipt=admission.admit(self.node,digest,probe=lambda path,sha: admission.probe_version(path,sha,retained_capsule=retained.append))
            self.assertEqual(snapshots.call_count,1)
        self.assertEqual(len(retained),1)
        self.assertEqual(retained[0].read_bytes(),self.node.read_bytes())
        self.assertEqual(retained[0].stat().st_mode & 0o777,0o500)
        self.assertEqual(receipt["path"],str(self.node))
        self.assertEqual(admission.environment(receipt,retained[0])["O3_ADMITTED_NODE"],str(retained[0]))

    def test_real_private_probe_clears_environment_and_snapshot_is_exact(self):
        audit=self.root/"environment.json"
        digest=self.program("import os,json\nfrom pathlib import Path\nPath("+repr(str(audit))+").write_text(json.dumps(dict(os.environ)))\nprint('v26.13.0')")
        with mock.patch.dict(os.environ,{"O3_PARENT_SECRET":"must-not-be-inherited"}):
            receipt=admission.admit(self.node,digest)
        child=json.loads(audit.read_text())
        self.assertNotIn("O3_PARENT_SECRET",child)
        self.assertEqual(child["PATH"],"")
        self.assertEqual(pathlib.Path(child["HOME"]).stat().st_mode & 0o777,0o700)
        target=self.root/"immutable"/"node"
        record=admission.snapshot(receipt,target)
        self.assertEqual(record["sha256"],digest)
        self.assertEqual(target.read_bytes(),self.node.read_bytes())
        self.assertEqual(target.stat().st_mode & 0o777,0o500)
        self.assertEqual(admission.environment(receipt,target),dict(zip(admission.ENV_KEYS,(str(target),digest,admission.VERSION))))

    def test_hash_failure_never_probes_or_changes_source(self):
        before=self.node.read_bytes()
        called=[]
        with self.assertRaisesRegex(ValueError,"hash mismatch"):
            admission.admit(self.node,"0"*64,probe=lambda *args:called.append(args))
        self.assertEqual(called,[])
        self.assertEqual(self.node.read_bytes(),before)

    def test_private_symlink_and_fifo_inputs_are_rejected_before_probe(self):
        link=self.root/"node-link"
        link.symlink_to(self.node)
        directory=self.root/"alias"
        directory.symlink_to(self.root,target_is_directory=True)
        fifo=self.root/"fifo"
        os.mkfifo(fifo,0o700)
        for path in [link,directory/"node",fifo]:
            with self.subTest(path=path.name),self.assertRaises((ValueError,OSError)):
                admission.admit(path,probe=lambda *args:self.fail("Unexpected probe"))
        self.assertTrue(self.node.is_file())

    def test_changed_or_replaced_executable_cannot_publish_a_receipt(self):
        def mutate(path,_digest):
            path.write_text(path.read_text()+"# changed\n")
            return admission.VERSION
        with self.assertRaisesRegex(ValueError,"changed"):
            admission.admit(self.node,probe=mutate)
        before=self.node.read_bytes()
        def replace(path,_digest):
            path.rename(self.root/"retained-original")
            path.write_bytes(before)
            path.chmod(0o700)
            return admission.VERSION
        with self.assertRaisesRegex(ValueError,"changed"):
            admission.admit(self.node,probe=replace)
        self.assertEqual((self.root/"retained-original").read_bytes(),before)

    def test_version_and_output_bounds_fail_closed(self):
        with self.assertRaisesRegex(ValueError,"version"):
            admission.admit(self.node,probe=lambda *_:"18.0.0")
        digest=self.program("print('v26.13.0'+'x'*1000)")
        with self.assertRaisesRegex(RuntimeError,"exceeds"):
            admission.admit(self.node,digest)

    def test_snapshot_never_overwrites_an_unknown_destination(self):
        receipt=admission.admit(self.node,probe=lambda *_:admission.VERSION)
        directory=self.root/"unknown"
        directory.mkdir()
        existing=directory/"node"
        existing.write_text("retained unknown entry")
        with self.assertRaises(FileExistsError):
            admission.snapshot(receipt,existing)
        self.assertEqual(existing.read_text(),"retained unknown entry")

    def test_relative_newline_and_privileged_entries_are_refused(self):
        for path in [pathlib.Path("node"),self.root/"line\nbreak"]:
            with self.assertRaises(ValueError):
                admission.admit(path,probe=lambda *_:admission.VERSION)
        self.node.chmod(0o4700)
        with self.assertRaises(ValueError):
            admission.admit(self.node,probe=lambda *_:admission.VERSION)


if __name__=="__main__":
    unittest.main()
