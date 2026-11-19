"""Provisional child-side SDK for a supervised source-adapter/1 process.

Adapters own source reads and resumable cursors. They never receive a Store or
receiver token. The host owns source identity, admission, capture policy and
the durable acknowledgment. stdout is exclusively the framed data channel.
"""
from __future__ import annotations

import sys
from dataclasses import dataclass
from typing import BinaryIO, Protocol

from .plugin_frames import MAX_FRAME_BYTES, read_frame, write_frame
from .plugin_manifest import PLUGIN_ID
from .plugin_wire import (MAX_BATCH_RECORDS, MAX_CONTROL_FRAME_BYTES, MAX_CURSOR_CHARS, MAX_ID_CHARS,
                          PROTOCOL_MAJOR, PollBatch, ProtocolError, _text,
                          decode_message, encode_message, validate_batch)

@dataclass(frozen=True)
class PollRequest:
    source_id: str
    cursor: str
    max_records: int
    max_bytes: int


class SourceAdapter(Protocol):
    def poll(self, request: PollRequest) -> PollBatch: ...
    def ack(self, cursor: str) -> None: ...


class AdapterServer:
    """One connection state machine; a protocol error poisons the connection."""
    def __init__(self, plugin_id: str, adapter: SourceAdapter):
        if not isinstance(plugin_id, str) or not PLUGIN_ID.fullmatch(plugin_id):
            raise ValueError("invalid plugin ID")
        self.plugin_id = plugin_id
        self.adapter = adapter
        self.source_id: str | None = None
        self.host_limit = MAX_FRAME_BYTES
        self.pending: str | None = None
        self.stopped = False

    def handle(self, message: dict) -> tuple[dict | None, int]:
        if not isinstance(message, dict) or type(message.get("protocol")) is not int or \
                message["protocol"] != PROTOCOL_MAJOR:
            raise ProtocolError("plugin control protocol mismatch")
        kind = message.get("type")
        if self.source_id is None:
            if set(message) != {"type", "protocol", "plugin_id", "source_id", "max_frame_bytes"}:
                raise ProtocolError("invalid plugin hello fields")
            if kind != "hello" or message["plugin_id"] != self.plugin_id:
                raise ProtocolError("plugin hello identity mismatch")
            self.source_id = _text(message["source_id"], "source ID", MAX_ID_CHARS)
            limit = message["max_frame_bytes"]
            if type(limit) is not int or not 1 <= limit <= MAX_FRAME_BYTES:
                raise ProtocolError("invalid host frame budget")
            self.host_limit = limit
            return ({"type": "hello_ack", "protocol": PROTOCOL_MAJOR,
                     "plugin_id": self.plugin_id, "features": ["source-adapter/1"]}, limit)
        if kind == "stop":
            if set(message) != {"type", "protocol"}:
                raise ProtocolError("invalid plugin stop fields")
            self.stopped = True
            return None, self.host_limit
        if kind == "poll":
            if set(message) != {"type", "protocol", "source_id", "cursor",
                                 "max_records", "max_bytes"}:
                raise ProtocolError("invalid plugin poll fields")
            if message["source_id"] != self.source_id or self.pending is not None:
                raise ProtocolError("plugin source mismatch or unacknowledged batch")
            cursor = message["cursor"]
            if cursor != "":
                cursor = _text(cursor, "cursor", MAX_CURSOR_CHARS)
            if not isinstance(cursor, str):
                raise ProtocolError("invalid cursor")
            count, size = message["max_records"], message["max_bytes"]
            if (type(count) is not int or not 1 <= count <= MAX_BATCH_RECORDS
                    or type(size) is not int or not 1 <= size <= self.host_limit):
                raise ProtocolError("invalid plugin poll budget")
            request = PollRequest(self.source_id, cursor, count, size)
            batch = self.adapter.poll(request)
            if not isinstance(batch, PollBatch):
                raise ProtocolError("adapter must return a PollBatch")
            validated = validate_batch({"type": "batch", "protocol": PROTOCOL_MAJOR,
                                        "source_id": batch.source_id, "cursor": batch.cursor,
                                        "records": list(batch.records)}, source_id=self.source_id)
            if len(validated.records) > count or validated.records and validated.cursor == cursor:
                raise ProtocolError("adapter exceeded budget or did not advance cursor")
            response = {"type": "batch", "protocol": PROTOCOL_MAJOR,
                        "source_id": self.source_id, "cursor": validated.cursor,
                        "records": list(validated.records)}
            if len(encode_message(response).encode("utf-8")) > size:
                raise ProtocolError("adapter exceeded poll byte budget")
            self.pending = validated.cursor
            return response, size
        if kind == "ack":
            if set(message) != {"type", "protocol", "source_id", "cursor"}:
                raise ProtocolError("invalid plugin acknowledgment fields")
            if message["source_id"] != self.source_id or message["cursor"] != self.pending:
                raise ProtocolError("plugin acknowledgment mismatch")
            self.adapter.ack(self.pending)
            self.pending = None
            return None, self.host_limit
        raise ProtocolError("unknown plugin control message")

    def serve(self, input_stream: BinaryIO, output_stream: BinaryIO) -> None:
        while not self.stopped:
            raw = read_frame(input_stream, max_bytes=MAX_CONTROL_FRAME_BYTES)
            if raw is None:
                return
            response, limit = self.handle(decode_message(raw))
            if response is not None:
                write_frame(output_stream, encode_message(response), max_bytes=limit)


def run_adapter(plugin_id: str, adapter: SourceAdapter) -> None:
    """Serve one host connection on binary stdin/stdout without text output."""
    AdapterServer(plugin_id, adapter).serve(sys.stdin.buffer, sys.stdout.buffer)
