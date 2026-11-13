"""Synthetic source-projection microbenchmark, not complete-monitor economics.

Run with the repository Python. No receiver, native discovery or provider calls.
Compare three consumers per immutable generation with and without app-owned
preparation, using canonical titles and a separate prompt-fallback case.
"""
import argparse
from dataclasses import replace
import hashlib
import json
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import time

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from observatory.agent_hierarchy import canonical_agent_id
from observatory.terminal import Snapshot
from observatory.terminal_ui import SourcePreparation, sources


def fixture(fallback):
    sessions=[];activity=[]
    for index in range(50):
        actor=canonical_agent_id('Codex','synthetic',str(index))
        task=f'Repair token refresh path {index}, preserve cancellation and verify the regression.'
        sessions.append(dict(source_id=f'source-{index}',session_id=f'session-{index}',
            agent_id=actor,service='codex',runtime='Codex',project='synthetic',
            task=task,initial_task='' if fallback else task,run_state='completed',
            last_record_ns=1,capture_content=True))
        activity.append(dict(id=f'prompt-{index}',source_id=f'prompt-{index}',
            agent_id=actor,session_id=f'session-{index}',service='codex',
            domain='message',action='user',subject=task,detail=json.dumps({'text':task}),
            time_unix_nano=1))
        for offset in range(3):
            activity.append(dict(id=f'work-{index}-{offset}',agent_id=actor,
                session_id=f'session-{index}',service='codex',domain='shell',action='run',
                subject='python -m unittest',detail='Synthetic captured output. '*20,
                time_unix_nano=offset+2,result='success'))
    return Snapshot(local_status={'sessions':sessions},activity=activity,receiver_id='synthetic')


def measure(snapshot,generations,consumers,cached):
    cache=SourcePreparation() if cached else None
    start=time.perf_counter()
    for generation in range(generations):
        current=replace(snapshot,updated=generation+1)
        for _ in range(consumers):sources(current,preparation=cache)
    return (time.perf_counter()-start)*1000


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--samples',type=int,default=7)
    parser.add_argument('--generations',type=int,default=20)
    parser.add_argument('--consumers',type=int,default=3)
    args=parser.parse_args()
    if min(args.samples,args.generations,args.consumers)<1:parser.error('Counts must be positive')
    result=dict(kind='source-projection-microbenchmark',python=platform.python_version(),
        source=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        source_dirty=bool(subprocess.check_output(['git','status','--porcelain','--untracked-files=no'],text=True)),
        script_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        samples=args.samples,generations=args.generations,consumers=args.consumers,cases={})
    for name,fallback in [('canonical_title',False),('prompt_fallback',True)]:
        snapshot=fixture(fallback);cache=SourcePreparation()
        expected=sources(snapshot)
        assert expected==sources(snapshot,preparation=cache)==sources(snapshot,preparation=cache)
        assert len(cache.entries)==1,'Fixture exceeds admission budget'
        # Warm both paths; alternate ordering to reduce simple order bias.
        measure(snapshot,1,args.consumers,False);measure(snapshot,1,args.consumers,True)
        values={False:[],True:[]}
        for sample in range(args.samples):
            for cached in ([False,True] if sample%2==0 else [True,False]):
                values[cached].append(round(measure(snapshot,args.generations,args.consumers,cached),3))
        result['cases'][name]=dict(source_rows=50,activity_rows=200,equivalent=True,
            uncached_ms=values[False],prepared_ms=values[True],
            uncached_median_ms=statistics.median(values[False]),
            prepared_median_ms=statistics.median(values[True]))
    print(json.dumps(result,indent=2))


if __name__=='__main__':main()
