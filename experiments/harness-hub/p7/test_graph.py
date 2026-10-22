"""Exercise the shipped RSI Graph's Ops without a provider: each Op's script
is taken from the Graph and run as the sandbox would, with its inputs under
a temporary /in and its grants under a temporary /local-inputs. Profile and
verify read experiment 1 group B through the built data hub, when both are
there."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
GRAPH = json.loads((REPO / 'examples/graphs/rsi-signals.json').read_text())
HUB = REPO / 'rust/target/debug/anchor-hub'
DATA = REPO / '.local/harness-hub-p6/exp1/B'
sys.path.insert(0, str(HERE / 'graph'))
import build  # noqa: E402

needs_data = unittest.skipUnless(HUB.is_file() and DATA.is_dir(), 'needs the built hub and exp1/B')


class Op:
    """A temporary sandbox layout for one Op."""

    def __init__(self):
        self.root = Path(tempfile.mkdtemp())
        self.inputs, self.local, self.work = (self.root / n for n in ('in', 'local', 'work'))
        for path in (self.inputs, self.local / 'hub-bin', self.work):
            path.mkdir(parents=True)
        if HUB.is_file():
            shutil.copy(HUB, self.local / 'hub-bin/anchor-hub')
        if DATA.is_dir():
            (self.local / 'harness-hub-data').symlink_to(DATA)
        (self.local / 'host-source').mkdir()

    def give(self, node, name, value):
        path = self.inputs / node / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value if isinstance(value, str) else json.dumps(value))

    def run(self, name):
        env = dict(os.environ, RSI_IN=str(self.inputs), RSI_LOCAL=str(self.local))
        return subprocess.run(['sh', '-c', GRAPH['ops'][name]['run']], cwd=self.work,
                              env=env, capture_output=True, text=True)

    def read(self, name):
        text = (self.work / name).read_text()
        return json.loads(text) if name.endswith('.json') else text

    def close(self):
        shutil.rmtree(self.root)


def proposal(pid='P1', **changes):
    value = {'id': pid, 'title': 'make python3 start', 'level': 2,
             'change': 'mount /etc/alternatives in the sandbox',
             'claims': ['investigate-1:c1'],
             'metric': {'name': 'not_started python3 commands', 'baseline': '74', 'target': '0'},
             'guardrails': ['any task mean_score below its baseline'],
             'scope': 'all tasks', 'quality': 'non_decreasing',
             'validation': 'experiment.py two arms + anchor-hub gate'}
    value.update(changes)
    return value


class Built(unittest.TestCase):
    def test_the_shipped_graph_is_what_build_writes(self):
        self.assertEqual((REPO / 'examples/graphs/rsi-signals.json').read_text(), build.text(),
                         'run experiments/harness-hub/p7/graph/build.py')

    def test_every_node_with_grants_exists_and_ops_read_what_they_need(self):
        ids = {n['id'] for n in GRAPH['nodes']}
        self.assertLessEqual(set(build.GRANTS), ids)
        self.assertEqual(GRAPH['entry'], 'profile')


class Gate(unittest.TestCase):
    def setUp(self):
        self.op = Op()
        self.op.give('verify', 'verified.json', {'claims': [
            {'id': 'investigate-1:c1', 'verdict': 'verified'},
            {'id': 'investigate-2:c1', 'verdict': 'refuted'}]})

    def tearDown(self):
        self.op.close()

    def gate(self, proposals, verdicts, earlier=None):
        self.op.give('synthesize', 'proposals.json', {'proposals': proposals})
        self.op.give('review', 'review.json', {'summary': 's', 'proposals': verdicts})
        if earlier is not None:
            self.op.give('gate', 'gate.json', earlier)
        done = self.op.run('gate')
        self.assertEqual(done.returncode, 0, done.stderr)
        return done.stdout.strip().splitlines()[-1], self.op.read('gate.json')

    def status(self, decision):
        return {item['id']: item['status'] for item in decision['proposals']}

    def test_sound_proposals_kept_by_the_review_are_published(self):
        route, decision = self.gate([proposal()], [{'id': 'P1', 'verdict': 'keep'}])
        self.assertEqual(route, 'ANCHOR_ROUTE: publish')
        self.assertEqual(self.status(decision), {'P1': 'accepted'})

    def test_only_the_proposals_with_problems_go_back(self):
        route, decision = self.gate(
            [proposal('P1'), proposal('P2', claims=['investigate-2:c1']), proposal('P3')],
            [{'id': 'P1', 'verdict': 'keep'}, {'id': 'P2', 'verdict': 'keep'},
             {'id': 'P3', 'verdict': 'revise', 'blocking': ['the target is two thresholds']}])
        self.assertEqual(route, 'ANCHOR_ROUTE: synthesize')
        self.assertEqual(self.status(decision), {'P1': 'accepted', 'P2': 'revise', 'P3': 'revise'})
        p2 = next(i for i in decision['proposals'] if i['id'] == 'P2')
        self.assertIn('claim investigate-2:c1 is refuted', ' '.join(p2['structure']))

    def test_the_second_pass_publishes_what_is_left_with_its_status(self):
        route, decision = self.gate(
            [proposal('P1'), proposal('P2', quality='decreasing'), proposal('P3')],
            [{'id': 'P1', 'verdict': 'keep'}, {'id': 'P2', 'verdict': 'keep'},
             {'id': 'P3', 'verdict': 'reject'}], earlier={'pass': 1})
        self.assertEqual(route, 'ANCHOR_ROUTE: publish')
        self.assertEqual(self.status(decision), {'P1': 'accepted', 'P2': 'unresolved', 'P3': 'rejected'})

    def test_a_proposal_the_review_skipped_is_not_accepted(self):
        _, decision = self.gate([proposal()], [])
        self.assertEqual(self.status(decision), {'P1': 'revise'})

    def test_an_unreadable_proposals_file_goes_back_once_then_publishes(self):
        self.op.give('synthesize', 'proposals.json', 'not json')
        self.op.give('review', 'review.json', {'proposals': []})
        first = self.op.run('gate')
        self.assertTrue(first.stdout.strip().endswith('synthesize'), first.stdout + first.stderr)
        self.op.give('gate', 'gate.json', self.op.read('gate.json'))
        second = self.op.run('gate')
        self.assertTrue(second.stdout.strip().endswith('publish'), second.stdout + second.stderr)


class Publish(unittest.TestCase):
    def test_every_proposal_appears_with_its_status_and_missing_parts_are_named(self):
        op = Op()
        try:
            op.give('synthesize', 'proposals.json', {'proposals': [proposal('P1'), proposal('P2')]})
            op.give('synthesize', 'synthesis.md', '# synthesis\n')
            op.give('gate', 'gate.json', {'pass': 2, 'proposals': [
                {'id': 'P1', 'status': 'accepted', 'structure': [], 'review': []},
                {'id': 'P2', 'status': 'unresolved', 'structure': ['metric.target is empty'], 'review': []}]})
            done = op.run('publish')
            self.assertEqual(done.returncode, 0, done.stderr)
            report = op.read('report.md')
            self.assertIn('P1 [accepted]', report)
            self.assertIn('P2 [unresolved]', report)
            self.assertIn('open: metric.target is empty', report)
            self.assertIn('verified.md is missing', report)
            self.assertEqual([p['status'] for p in op.read('proposals.json')['proposals']],
                             ['accepted', 'unresolved'])
        finally:
            op.close()


@needs_data
class OnRecordedData(unittest.TestCase):
    def test_the_profile_shows_what_the_first_blind_run_could_not_see(self):
        op = Op()
        try:
            done = op.run('profile')
            self.assertEqual(done.returncode, 0, done.stderr)
            page = op.read('profile.md')
            self.assertIn('74 of 74 python3 command(s) never started', page)
            self.assertIn('check phones failed in 3 of 10', page)
            self.assertIn('were stopped at the time limit', page)
            self.assertIn('task contacts v1', page)
            self.assertLess(len(page), 20000)
            self.assertEqual(op.read('signals.json')['signals'][0]['code'], 'check_failing')
        finally:
            op.close()

    def test_verify_re_runs_checks_and_judges_each_claim(self):
        op = Op()
        try:
            source = op.local / 'host-source/node_tools.rs'
            source.write_text('line one\nconst DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);\n')
            python3 = {'tool': 'report', 'args': {'analyzer': 'health'},
                       'path': '/summary/tools/anchor_run/commands/python3/not_started'}
            op.give('investigate-1', 'findings-1.json', {'claims': [
                {'id': 'c1', 'kind': 'observed', 'text': 'python3 never starts',
                 'checks': [dict(python3, op='==', value=74),
                            {'file': str(source), 'lines': [2, 2], 'contains': 'from_secs(30)'}]},
                {'id': 'c2', 'kind': 'observed', 'text': 'wrong count', 'checks': [dict(python3, op='==', value=3)]},
                {'id': 'c3', 'kind': 'inferred', 'text': 'no check', 'rests_on': ['c1']},
                {'id': 'c4', 'kind': 'observed', 'text': 'bad tool', 'checks': [{'tool': 'nope'}]},
                {'id': 'c5', 'kind': 'observed', 'text': 'outside', 'checks': [{'file': '/etc/passwd', 'contains': 'root'}]}]})
            op.give('investigate-2', 'findings-2.json', {'skip': True})
            done = op.run('verify')
            self.assertEqual(done.returncode, 0, done.stderr)
            verdicts = {c['id']: c['verdict'] for c in op.read('verified.json')['claims']}
            self.assertEqual(verdicts, {
                'investigate-1:c1': 'verified', 'investigate-1:c2': 'refuted',
                'investigate-1:c3': 'unverified', 'investigate-1:c4': 'unverified',
                'investigate-1:c5': 'unverified'})
            self.assertIn('| investigate-1:c2 | refuted |', op.read('verified.md'))
        finally:
            op.close()


if __name__ == '__main__':
    unittest.main()
