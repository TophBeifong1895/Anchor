"""Exercise the shipped Graph's publication gate without a provider."""

import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[3]
GRAPH = json.loads((REPO / 'examples/graphs/rsi-signals.json').read_text())


def proposal(quality='non_decreasing'):
    return {'level': 1, 'signal_id': 'fixture', 'change': 'change one tool subset',
            'evidence': ['fixture evidence key'],
            'prediction': {'metric': 'tool failure rate', 'coordinates': 'fixture node',
                           'baseline': '10%', 'expected': '0%', 'quality': quality},
            'validation': 'contrast on a controlled fixture; quality must not decline'}


class PublicationGate(unittest.TestCase):
    def gate(self, decision, proposals):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'review.md').write_text(decision + '\nreview evidence\n')
            (root / 'proposals.json').write_text(json.dumps({'proposals': proposals}))
            command = GRAPH['ops']['gate']['run']
            command = command.replace('/in/review/review.md', str(root / 'review.md'))
            command = command.replace('/in/synthesize/proposals.json', str(root / 'proposals.json'))
            result = subprocess.run(['sh', '-c', command], cwd=root, capture_output=True, text=True)
            return result, (root / 'gate.txt').read_text()

    def test_a_valid_prediction_routes_to_publication(self):
        result, feedback = self.gate('PASS', [proposal()])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'ANCHOR_ROUTE: publish')
        self.assertTrue(feedback.startswith('PASS\n'))

    def test_quality_loss_cannot_be_overridden_by_a_pass(self):
        result, feedback = self.gate('PASS', [proposal('decreasing')])
        self.assertEqual(result.stdout.strip(), 'ANCHOR_ROUTE: synthesize')
        self.assertIn('quality loss', feedback)

    def test_missing_prediction_returns_actionable_feedback(self):
        candidate = proposal()
        del candidate['prediction']['coordinates']
        result, feedback = self.gate('PASS', [candidate])
        self.assertEqual(result.stdout.strip(), 'ANCHOR_ROUTE: synthesize')
        self.assertIn('coordinates', feedback)

    def test_reviewer_revision_returns_to_synthesis(self):
        result, feedback = self.gate('REVISE', [proposal()])
        self.assertEqual(result.stdout.strip(), 'ANCHOR_ROUTE: synthesize')
        self.assertIn('review evidence', feedback)

    def test_a_block_stops_without_publishing(self):
        result, feedback = self.gate('BLOCK', [proposal()])
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('ANCHOR_ROUTE:', result.stdout)
        self.assertTrue(feedback.startswith('BLOCK\n'))

    def test_unknown_quality_stays_explicitly_unvalidated(self):
        result, feedback = self.gate('PASS', [proposal('unknown')])
        self.assertEqual(result.stdout.strip(), 'ANCHOR_ROUTE: publish')
        self.assertIn('unvalidated', feedback)

    @unittest.skipUnless(shutil.which('bwrap'), 'bubblewrap is not installed')
    def test_gate_runs_without_the_system_python_alternatives_mount(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('input/review', 'input/synthesize', 'work'):
                (root / name).mkdir(parents=True)
            (root / 'input/review/review.md').write_text('PASS\nfixture review\n')
            (root / 'input/synthesize/proposals.json').write_text(
                json.dumps({'proposals': [proposal()]}))
            command = ['bwrap', '--unshare-all', '--die-with-parent',
                       '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin',
                       '--symlink', 'usr/lib', '/lib', '--symlink', 'usr/lib64', '/lib64',
                       '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
                       '--ro-bind', str(root / 'input'), '/in',
                       '--bind', str(root / 'work'), '/workspace', '--chdir', '/workspace',
                       '/bin/sh', '-c', GRAPH['ops']['gate']['run']]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), 'ANCHOR_ROUTE: publish')
            self.assertTrue((root / 'work/gate.txt').read_text().startswith('PASS\n'))


if __name__ == '__main__':
    unittest.main()
