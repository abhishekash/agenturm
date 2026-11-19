"""Bounded wire framing shared with the future Rust plugin host.

Framing does not validate JSON, grant access, or acknowledge a source cursor.
After any framing error the caller must close the connection; resynchronizing
an untrusted byte stream could admit bytes under a false frame boundary.
"""
from __future__ import annotations

from typing import BinaryIO


MAX_FRAME_BYTES = 4 * 1024 * 1024


class FrameError(ValueError):
    pass


def _limit(value: int) -> int:
    if type(value) is not int or value <= 0:
        raise FrameError("frame limit must be a positive integer")
    return min(value, MAX_FRAME_BYTES)


def _read_exact(stream: BinaryIO, count: int) -> bytes:
    result = bytearray()
    while len(result) < count:
        chunk = stream.read(count - len(result))
        if not isinstance(chunk, bytes):
            raise FrameError("binary stream returned non-bytes")
        if not chunk:
            raise FrameError("frame ended before its declared length")
        result.extend(chunk)
    return bytes(result)


def _write_all(stream: BinaryIO, raw: bytes) -> None:
    remaining = memoryview(raw)
    while remaining:
        written = stream.write(remaining)
        if type(written) is not int or written <= 0 or written > len(remaining):
            raise FrameError("binary stream did not make write progress")
        remaining = remaining[written:]


def read_frame(stream: BinaryIO, *, max_bytes: int = MAX_FRAME_BYTES) -> str | None:
    """Read one UTF-8 frame; `None` means EOF before the first prefix byte."""
    limit = _limit(max_bytes)
    first = stream.read(1)
    if not isinstance(first, bytes):
        raise FrameError("binary stream returned non-bytes")
    if first == b"":
        return None
    size = int.from_bytes(first + _read_exact(stream, 3), "big")
    if size == 0:
        raise FrameError("empty frame is forbidden")
    if size > limit:
        raise FrameError(f"frame of {size} bytes exceeds {limit}")
    try:
        return _read_exact(stream, size).decode("utf-8", "strict")
    except UnicodeDecodeError as exc:
        raise FrameError("frame is not UTF-8") from exc


def write_frame(stream: BinaryIO, payload: str, *, max_bytes: int = MAX_FRAME_BYTES) -> None:
    """Write one frame. An I/O error poisons the connection."""
    limit = _limit(max_bytes)
    if not isinstance(payload, str):
        raise FrameError("frame payload must be text")
    if len(payload) > limit:
        raise FrameError(f"frame exceeds {limit} bytes")
    try:
        raw = payload.encode("utf-8", "strict")
    except UnicodeEncodeError as exc:
        raise FrameError("frame is not UTF-8") from exc
    if not raw:
        raise FrameError("empty frame is forbidden")
    if len(raw) > limit:
        raise FrameError(f"frame of {len(raw)} bytes exceeds {limit}")
    _write_all(stream, len(raw).to_bytes(4, "big"))
    _write_all(stream, raw)
    stream.flush()
