"""What every evaluation task shares: its shape, the seed of its inputs, the
commands its agent may run and how it finishes."""

from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
SEED = 20261021
AGENT = 'agent'
# In the sandbox `/usr/bin/python3` points into /etc/alternatives, which is
# not mounted, so only the versioned interpreter runs.
PYTHON = 'python3.12'
COMMANDS = ('sh,cat,printf,grep,test,true,ls,mkdir,awk,sort,uniq,wc,head,tail,cut,tr,sed,'
            'find,jq,python3,' + PYTHON)
COMPLETION = ('When the outputs are written, read them back to check them. Then call final_result '
              'separately with route grade, a one-line summary, and an actual observed receipt as '
              'required by its schema.')


@dataclass
class Task:
    id: str
    version: int
    files: dict          # input path under data/ -> text
    instructions: str
    outputs: list        # files the agent writes to /workspace
    expected: dict       # what the grader compares against
    reference: dict      # output name -> text of a correct solution
    commands: str = COMMANDS
    # exact: one right answer, checked exactly; a capable baseline gets it,
    # so efficiency is the signal. graded: quality comes in degrees.
    tier: str = 'exact'
    # (function in generators.py, seed) when the prepare Op generates the
    # inputs instead of carrying them; `files` then holds bytes.
    generator: tuple = ()
