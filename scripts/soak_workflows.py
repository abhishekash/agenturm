"""Exercise live ingestion, receiver replacement, review bounds and durable workflows.

Uses private temporary databases and synthetic records. Discovery, provider calls
and desktop notifications are disabled. UI runs headlessly; ledger/store claims,
acknowledgements, retry and resolution are exercised separately with explicit
fixtures because the headless controller intentionally disables persistence.

Run: uv run python scripts/soak_workflows.py --duration 1200 --output /tmp/soak.json
Duration is real wall time, not a virtual-clock or multi-hour stability claim.
"""

import argparse, asyncio, importlib.util, json, math, sqlite3, sys, tempfile, threading, time, urllib.request
from contextlib import closing
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from observatory.terminal import Api, Feed, LocalReceiver
from observatory.terminal_ui import ObservatoryApp, ObservatoryScreen
from observatory.feed_requests import shared_get
from observatory.workflow_evidence import (
    ExecutionScope,
    ReviewCache,
    MAX_EVENTS,
    MAX_BYTES,
    CACHE_BYTES,
    CACHE_SELECTIONS,
)
from observatory.saved_tasks import TaskStore
from observatory.workflow_store import WorkflowStore
from observatory.attention import AttentionLedger
from scripts.benchmark_app import memory_mib


class CountApi(Api):
    def __init__(self, *args):
        super().__init__(*args)
        self.lock = threading.Lock()
        self.active = 0
        self.peak = 0
        self.reads = 0

    def _get(self, *args):
        with self.lock:
            self.active += 1
            self.peak = max(self.peak, self.active)
            self.reads += 1
        try:
            return super()._get(*args)
        finally:
            with self.lock:
                self.active -= 1


def batch(epoch, tick, *, old=False):
    records = []
    now = time.time_ns() - (2 * 86400 * 10**9 if old else 0)
    for i in range(6):
        attributes = {
            "agent.id": "ag1_" + f"{i + 1:032x}",
            "session.id": f"soak-family-{i // 2}",
            "agent.task": f"SOAK epoch {epoch} worker {i}",
            "gen_ai.tool.name": f"pytest tests/soak_{i}.py",
            "gen_ai.tool.type": "shell",
            "synthetic.payload": "x" * 1024,
        }
        if i % 2:
            attributes["agent.parent_id"] = "ag1_" + f"{i:032x}"
        records.append(
            dict(
                trace_id=f"{i + 1 + (1000 if old else 0):032x}",
                span_id=f"{tick % 96 + 1:016x}",
                name="synthetic.tool",
                start_unix_nano=now,
                end_unix_nano=now + 1000,
                status="OK",
                attributes=attributes,
                events=[
                    dict(
                        name="agent.activity",
                        time_unix_nano=now,
                        attributes={
                            "agent.activity.domain": "shell",
                            "agent.activity.action": "execute",
                            "agent.activity.subject": f"pytest tests/soak_{i}.py",
                            "agent.activity.result": "success",
                            "agent.activity.detail": json.dumps({"exit_code": 0}),
                        },
                    )
                ],
            )
        )
    return records


def post(api, records):
    req = urllib.request.Request(
        api.endpoint + "/api/spans",
        data=json.dumps({"service": "validation", "spans": records}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=5) as response:
        assert response.status == 200


def native_durable_cycle(root,snapshot,tick):
    """Use real eligibility/delivery and exact resolution, with fixture notifier."""
    from observatory.attention_runner import AttentionRunner
    state=WorkflowStore(root/'workflow.sqlite3');runner=AttentionRunner();runner.ledger.restore(state.load())
    stamp=time.time_ns()
    observed=SimpleNamespace(updated=time.time(),error='',read_errors={},receiver_id=snapshot.receiver_id,
        receiver_time_ns=stamp,indexing_ready=True,activity=[],sessions=[],traces=[],local_status={},
        native_lifecycle=[],native_lifecycle_warning='',native_lifecycle_prepared=True)
    base=dict(id='attention-fixture',service='validation-attention',agent_id='ag1_'+'f'*32,
        session_id='attention-fixture',trace_id='f'*32,state='Working',waiting_for='',task='SENSITIVE_SYNTHETIC_ATTENTION_TASK')
    runner.update([base],observed,{},store=state);runner.persist(state)
    pending=dict(schema=1,receiver_id=snapshot.receiver_id,sender_id='validation-attention',event_id=f'request-{tick}',
        sequence=tick*2+1,kind='approval_requested',request_id=f'request-{tick}',request_kind='approval_requested',
        source_clock='receiver',source_order='sender-sequence',time_unix_nano=time.time_ns(),source_id='fixture-source',
        service=base['service'],agent_id=base['agent_id'],session_id=base['session_id'],trace_id=base['trace_id'],turn_id='',resolved=False)
    observed.native_lifecycle=[pending];observed.receiver_time_ns=time.time_ns();observed.updated=time.time()
    runner.update([base],observed,{},store=state);runner.persist(state)
    incident=next(value for value in runner.ledger.incidents.values() if value.anchor=='request:'+pending['request_id'])
    attempts=[]
    def failed(*values):attempts.append('failed');return 'Desktop notification delivery failed'
    def delivered(*values):attempts.append('delivered');return 'Desktop notification delivered'
    runner.deliver(state,runner.delivery_candidates(),notify=failed)
    runner.deliver(state,runner.delivery_candidates(),notify=delivered)
    assert attempts==['failed'],'Automatic delivery retried without explicit intent'
    runner.deliver(state,[incident],notify=delivered,retry=True)
    assert attempts==['failed','delivered']
    state.acknowledge(incident.namespace,incident.id,incident.scope_key,action='seen')
    restarted=AttentionRunner();restarted.ledger.restore(WorkflowStore(root/'workflow.sqlite3').load())
    restarted.update([base],observed,{},store=state);restarted.persist(state)
    assert any(row[0].get('_attention_seen') for row in restarted.ledger.rows),'Seen request vanished on restart'
    # Missing metadata or an unrelated working status is not native resolution.
    observed.native_lifecycle=[]
    restarted.update([base],observed,{},store=state);restarted.persist(state)
    assert incident.id in restarted.ledger.incidents,'Working incorrectly resolved exact native request'
    observed.native_lifecycle=[{**pending,'event_id':f'resolved-{tick}','sequence':tick*2+2,
        'kind':'request_resolved','time_unix_nano':time.time_ns(),'resolved':True}]
    restarted.update([base],observed,{},store=state);restarted.persist(state)
    saved=next(row for row in state.load() if row['id']==incident.id)
    assert saved['resolved_at'] is not None and saved['acknowledgement']=='seen'
    assert saved['delivery_status']=='delivered'
    assert 'SENSITIVE_SYNTHETIC_ATTENTION_TASK' not in json.dumps(state.load())
    return dict(kind='AttentionRunner exact native lifecycle',notification_attempts=len(attempts),
        acknowledgement_survived_restart=True,working_did_not_resolve=True,exact_resolution=True,no_real_notifications=True)


def durable_cycle(root, snapshot, projection, scope, tick):
    if importlib.util.find_spec("observatory.attention_runner") is not None:
        native_durable_cycle(root,snapshot,tick)
    else:
        # Baseline capability is explicitly legacy; it does not validate native v1.
        state = WorkflowStore(root / "workflow.sqlite3")
        ledger = AttentionLedger()
        ledger.restore(state.load())
        # Headless UI intentionally disables durable controller writes. Exercise
        # the real ledger/store separately with an explicit, non-empty fixture.
        observed = SimpleNamespace(
            updated=time.time(),
            error="",
            read_errors={},
            receiver_id=snapshot.receiver_id,
            activity=[],
            sessions=[],
            traces=[],
            local_status={},
        )
        waiting = dict(
            id="attention-fixture",
            service="validation-attention",
            agent_id="ag1_" + "f" * 32,
            session_id="attention-fixture",
            trace_id="f" * 32,
            state="Waiting",
            waiting_for="Approval",
            request_id=f"request-{tick}",
            task="SENSITIVE_SYNTHETIC_ATTENTION_TASK",
        )
        ledger.update([waiting], observed, {}, time.time())
        result = state.observe(list(ledger.incidents.values()), resolved=ledger.resolved)
        assert not result.unpersisted
        incident = next(
            value
            for value in ledger.incidents.values()
            if value.anchor == "request:" + waiting["request_id"]
        )
        record = incident.to_record()
        assert state.claim(record["namespace"], record["id"], record["scope_key"])
        assert not WorkflowStore(root / "workflow.sqlite3").claim(
            record["namespace"], record["id"], record["scope_key"]
        )
        state.finish_delivery(
            record["namespace"],
            record["id"],
            status="failed",
            detail="Synthetic delivery only",
        )
        assert state.claim(
            record["namespace"], record["id"], record["scope_key"], retry=True
        )
        state.finish_delivery(
            record["namespace"],
            record["id"],
            status="delivered",
            detail="Synthetic delivery only",
        )
        state.acknowledge(
            record["namespace"], record["id"], record["scope_key"], action="seen"
        )
        restarted = AttentionLedger()
        restarted.restore(WorkflowStore(root / "workflow.sqlite3").load())
        restarted.update([waiting], observed, {}, time.time())
        assert any(row[0].get("_attention_seen") for row in restarted.rows), (
            "Seen request disappeared on restart"
        )
        restarted.update(
            [{**waiting, "state": "Working", "waiting_for": None}],
            observed,
            {},
            time.time(),
        )
        assert (record["namespace"], record["id"]) in restarted.resolved, (
            "Fresh positive evidence did not resolve request"
        )
        state.observe(list(restarted.incidents.values()), resolved=restarted.resolved)
        loaded = WorkflowStore(root / "workflow.sqlite3").load()
        saved = next(value for value in loaded if value["id"] == record["id"])
        assert saved["resolved_at"] is not None and saved["acknowledgement"] == "seen"
        assert len(loaded) <= 10000
        assert "SENSITIVE_SYNTHETIC_ATTENTION_TASK" not in json.dumps(loaded), (
            "Workflow receipts leaked task content"
        )
    store = TaskStore(root / "tasks.sqlite3")
    summaries = store.list()
    task = (
        store.get(summaries[0]["id"])
        if summaries
        else store.create("Synthetic continuity task")
    )
    task = store.update_brief(
        task["id"],
        {
            **task["brief"],
            "intent": "Retain exact synthetic evidence",
            "next_action": f"Continue after cycle {tick}",
        },
        expected_revision=task["revision"],
    )
    task = store.link(
        task["id"],
        dict(
            receiver=scope.receiver,
            producer=scope.producer,
            agent_id=scope.agent_id,
            session_id=scope.session_id,
            trace_id=scope.run_id,
            task="Synthetic selected work",
            review_snapshot=projection,
        ),
        expected_revision=task["revision"],
    )
    reopened = TaskStore(root / "tasks.sqlite3").get(task["id"])
    assert reopened["revision"] == task["revision"] and 0 < len(reopened["links"]) <= 20
    assert reopened["brief"]["intent"] == "Retain exact synthetic evidence"
    assert reopened["brief"]["next_action"] == f"Continue after cycle {tick}"
    assert any(
        link.get("receiver") == scope.receiver
        and link.get("agent_id") == scope.agent_id
        and link.get("trace_id") == scope.run_id
        and link.get("review_snapshot") == projection
        for link in reopened["links"]
    ), "Task reopen lost matching evidence"
    store.close()
    with closing(sqlite3.connect(root / "workflow.sqlite3")) as connection:
        return dict(
            incidents=connection.execute(
                "SELECT COUNT(*) FROM workflow_incidents_v1"
            ).fetchone()[0],
            acknowledgements=connection.execute(
                "SELECT COUNT(*) FROM workflow_acknowledgements_v1"
            ).fetchone()[0],
            claims=connection.execute(
                "SELECT COUNT(*) FROM workflow_notification_claims_v1"
            ).fetchone()[0],
            resolved=connection.execute(
                "SELECT COUNT(*) FROM workflow_incidents_v1 WHERE resolved_at IS NOT NULL"
            ).fetchone()[0],
        )


async def run(duration, output):
    setup = time.monotonic()
    samples = []
    cache = ReviewCache()
    epoch = 0
    cycles = 0
    replacements = 0
    outages = 0
    durable_stats = {}
    with tempfile.TemporaryDirectory(prefix="agenturm-live-soak-") as temporary:
        root = Path(temporary)
        receiver = LocalReceiver(root / "receiver-0.db", 0, None, local_agents=False)
        port = receiver.server.server_address[1]
        api = CountApi(f"http://127.0.0.1:{port}")
        feed = Feed(api)
        try:
            await asyncio.to_thread(post, api, batch(epoch, 0))
            await asyncio.to_thread(feed.refresh)
            app = ObservatoryApp(
                feed, persist_preferences=False, task_store_path=root / "tasks.sqlite3"
            )
            async with app.run_test(size=(120, 32)) as pilot:
                await pilot.pause()
                start = time.monotonic()
                setup_seconds = start - setup
                cpu = time.process_time()
                last_sample = start
                last_cpu = cpu
                next_report = start + 15
                targets = [duration / 3, 2 * duration / 3]
                sizes = [(120, 32), (80, 24), (60, 18), (45, 14), (160, 48)]
                while time.monotonic() - start < duration:
                    elapsed = time.monotonic() - start
                    if targets and elapsed >= targets[0]:
                        targets.pop(0)
                        while not isinstance(app.screen, ObservatoryScreen):
                            await pilot.press("escape")
                        previous = feed.snapshot.receiver_id
                        await asyncio.to_thread(receiver.close)
                        deadline = time.monotonic() + 10
                        while not feed.snapshot.error and time.monotonic() < deadline:
                            await asyncio.sleep(0.1)
                        assert feed.snapshot.error, (
                            "Disconnected receiver was not observed"
                        )
                        outages += 1
                        await pilot.press("f1", "escape")
                        epoch += 1
                        receiver = await asyncio.to_thread(
                            LocalReceiver,
                            root / f"receiver-{epoch}.db",
                            port,
                            None,
                            False,
                        )
                        await asyncio.to_thread(post, api, batch(epoch, 0))
                        feed.retry_now()
                        deadline = time.monotonic() + 20
                        while (
                            not feed.snapshot.receiver_id
                            or feed.snapshot.receiver_id == previous
                            or feed.snapshot.error
                        ) and time.monotonic() < deadline:
                            await asyncio.sleep(0.2)
                        assert (
                            feed.snapshot.receiver_id
                            and feed.snapshot.receiver_id != previous
                            and not feed.snapshot.error
                        ), "Receiver replacement failed to recover"
                        deadline = time.monotonic() + 20
                        while (
                            not feed.snapshot.sessions
                            or not all(
                                f"SOAK epoch {epoch} worker"
                                in str(row.get("latest_task", ""))
                                for row in feed.snapshot.sessions
                            )
                        ) and time.monotonic() < deadline:
                            await asyncio.sleep(0.2)
                        assert feed.snapshot.sessions and all(
                            f"SOAK epoch {epoch} worker"
                            in str(row.get("latest_task", ""))
                            for row in feed.snapshot.sessions
                        ), "Non-empty fresh receiver roster did not arrive"
                        replacements += 1
                    await asyncio.to_thread(post, api, batch(epoch, cycles))
                    if cycles % 5 == 0:
                        while not isinstance(app.screen, ObservatoryScreen):
                            await pilot.press("escape")
                        await pilot.resize_terminal(*sizes[(cycles // 5) % len(sizes)])
                        if app.size.width >= 60:
                            await pilot.press(
                                "1",
                                "enter",
                                "ctrl+right",
                                "up",
                                "f1",
                                "escape",
                                "escape",
                            )
                    scope = ExecutionScope(
                        feed.snapshot.receiver_id,
                        "validation",
                        "ag1_" + f"{cycles % 6 + 1:032x}",
                        f"soak-family-{(cycles % 6) // 2}",
                        f"{cycles % 6 + 1:032x}",
                        kind="run",
                    )
                    history = cache.get(scope, cycles)
                    response = await asyncio.to_thread(
                        shared_get,
                        api,
                        scope.path().replace("limit=200", "limit=20"),
                        review=True,
                    )
                    history.accept(response)
                    assert history.events and history.receiver_confirmed, (
                        "Scoped review returned no confirmed evidence"
                    )
                    assert all(
                        scope.matches(event) for event in history.events.values()
                    )
                    cache.trim()
                    assert (
                        len(cache.entries) <= CACHE_SELECTIONS
                        and sum(h.payload_bytes for h in cache.entries.values())
                        <= CACHE_BYTES
                    )
                    assert all(
                        len(h.events) <= MAX_EVENTS and h.payload_bytes <= MAX_BYTES
                        for h in cache.entries.values()
                    )
                    projection = history.projection(
                        {"task": "Synthetic selected work", "state": "Completed"}
                    )
                    durable_stats = await asyncio.to_thread(
                        durable_cycle,
                        root,
                        feed.snapshot,
                        projection,
                        scope,
                        cycles,
                    )
                    if cycles % 20 == 0:
                        await asyncio.to_thread(post, api, batch(epoch, 0, old=True))
                        deleted = await asyncio.to_thread(receiver.store.prune, 1)
                        assert deleted["spans"] > 0, (
                            "Retention failed to remove obsolete fixture"
                        )
                    cycles += 1
                    now = time.monotonic()
                    if now >= next_report:
                        sample = dict(
                            elapsed_seconds=round(now - start, 2),
                            cycles=cycles,
                            rss_mib=memory_mib(),
                            open_fds=len(list(Path("/proc/self/fd").iterdir()))
                            if Path("/proc/self/fd").is_dir()
                            else None,
                            threads=threading.active_count(),
                            interval_cpu_percent=round(
                                (time.process_time() - last_cpu)
                                / (now - last_sample)
                                * 100,
                                2,
                            ),
                            api_reads=api.reads,
                            peak_active_reads=api.peak,
                            cache_entries=len(cache.entries),
                            cache_bytes=sum(
                                h.payload_bytes for h in cache.entries.values()
                            ),
                            receiver_replacements=replacements,
                            durable=durable_stats,
                        )
                        if sample["open_fds"] is not None:
                            assert sample["open_fds"] < 128, (
                                "File descriptor budget exceeded"
                            )
                        samples.append(sample)
                        print(json.dumps(sample), file=sys.__stdout__, flush=True)
                        output.with_suffix(".progress.json").write_text(
                            json.dumps(sample, indent=2)
                        )
                        last_sample = now
                        last_cpu = time.process_time()
                        next_report = now + 30
                    await asyncio.sleep(2)
                measured = time.monotonic() - start
                for db in root.glob("*.sqlite3"):
                    with closing(sqlite3.connect(db)) as connection:
                        assert (
                            connection.execute("PRAGMA integrity_check").fetchone()[0]
                            == "ok"
                        )
                for db in root.glob("receiver-*.db"):
                    with closing(sqlite3.connect(db)) as connection:
                        assert (
                            connection.execute("PRAGMA integrity_check").fetchone()[0]
                            == "ok"
                        )
                result = dict(
                    passed=True,
                    setup_seconds=round(setup_seconds, 2),
                    measured_seconds=round(measured, 2),
                    cycles=cycles,
                    receiver_replacements=replacements,
                    observed_outages=outages,
                    peak_active_reads=api.peak,
                    headless_monitor=True,
                    durable_modules_exercised_separately=True,
                    durable=durable_stats,
                    samples=samples,
                )
                assert api.peak <= 4, (
                    f"Read concurrency exceeded shared budget: {api.peak}"
                )
                assert (
                    outages == 2 and replacements == 2 and all(durable_stats.values())
                )
        finally:
            feed.close()
            receiver.close()
        assert not feed.thread.is_alive(), "Feed worker survived cleanup"
        assert not receiver.thread.is_alive(), "Receiver worker survived cleanup"
        result["cleanup_completed"] = True
        output.write_text(json.dumps(result, indent=2))
        print(
            json.dumps({k: v for k, v in result.items() if k != "samples"}),
            file=sys.__stdout__,
            flush=True,
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--duration",
        type=float,
        default=1200,
        help="Wall seconds, at least 30; default 1200",
    )
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="New JSON result path; progress uses a sibling .progress.json",
    )
    args = parser.parse_args()
    if not math.isfinite(args.duration) or args.duration < 30:
        parser.error("--duration must be finite and at least 30 seconds")
    if args.output.exists() or args.output.with_suffix(".progress.json").exists():
        parser.error("Choose a new output path; existing results are not overwritten")
    if not args.output.parent.is_dir():
        parser.error("Output directory must already exist")
    asyncio.run(run(args.duration, args.output))
