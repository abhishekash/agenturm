"""Native ingest scope of the shared in-process JSONL collector."""


def native_ingest_scopes():
    from ..native_ingest_scopes import NativeIngestScope

    return (NativeIngestScope("observatory.local_agents"),)
