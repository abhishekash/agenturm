"""Project bounded message text from known SDK telemetry shapes."""

from __future__ import annotations

import json
from collections.abc import Mapping

from ..otlp_transport import MAX_PAYLOAD_TEXT_BYTES

MAX_MESSAGE_JSON_BYTES = 64 * 1024
MAX_MESSAGES = 64
MAX_PARTS = 64


def join_text(parts):
    """Preserve complete text parts in order, within one capture slot."""
    result = []
    size = 0
    for index, part in enumerate(parts):
        if index >= MAX_PARTS:
            return None
        if not isinstance(part, str) or not part:
            continue
        if len(part) > MAX_PAYLOAD_TEXT_BYTES:
            return None
        try:
            size += len(part.encode("utf-8")) + bool(result)
        except UnicodeError:
            return None
        if size > MAX_PAYLOAD_TEXT_BYTES:
            return None
        result.append(part)
    return "\n".join(result) if result else None


def _sequence(value, limit):
    return value if isinstance(value, (list, tuple)) and len(value) <= limit else ()


def _json_value(value):
    if not isinstance(value, str) or len(value) > MAX_MESSAGE_JSON_BYTES:
        return None
    try:
        if len(value.encode("utf-8")) <= MAX_MESSAGE_JSON_BYTES:
            return json.loads(value)
    except (ValueError, UnicodeError, RecursionError):
        pass
    return None


def _standard_parts(messages):
    for message in _sequence(messages, MAX_MESSAGES):
        if not isinstance(message, Mapping) or message.get("role") not in ("user", "assistant"):
            continue
        for part in _sequence(message.get("parts"), MAX_PARTS):
            if isinstance(part, Mapping) and part.get("type") == "text":
                yield part.get("content")


def _adk_parts(messages):
    for message in _sequence(messages, MAX_MESSAGES):
        if not isinstance(message, Mapping) or message.get("role") not in ("user", "model"):
            continue
        for part in _sequence(message.get("parts"), MAX_PARTS):
            if isinstance(part, Mapping) and set(part) == {"text"}:
                yield part["text"]


def structured_message_text(attributes, scope_name):
    """Read typed message slots from GenAI or the ADK legacy span attributes."""
    output = {}
    for direction in ("input", "output"):
        text = join_text(_standard_parts(_json_value(attributes.get(f"gen_ai.{direction}.messages"))))
        if text is not None:
            output[f"agenturm.payload.{direction}"] = text
    if output or scope_name != "gcp.vertex.agent":
        return output

    request = _json_value(attributes.get("gcp.vertex.agent.llm_request"))
    response = _json_value(attributes.get("gcp.vertex.agent.llm_response"))
    if isinstance(request, Mapping):
        text = join_text(_adk_parts(request.get("contents")))
        if text is not None:
            output["agenturm.payload.input"] = text
    if isinstance(response, Mapping):
        text = join_text(_adk_parts([response.get("content")]))
        if text is not None:
            output["agenturm.payload.output"] = text
    return output


def _agent_items(messages):
    for item in _sequence(messages, MAX_MESSAGES):
        if isinstance(item, Mapping):
            message = item.get("message", item)
            if not isinstance(message, Mapping) or message.get("role") not in ("user", "assistant"):
                continue
            if message.get("type") not in (None, "message"):
                continue
            content = message.get("content")
        elif getattr(item, "type", None) == "message" and getattr(item, "role", None) in (
            "user", "assistant",
        ):
            content = item.content
        else:
            continue
        if isinstance(content, str):
            yield content
            continue
        for part in _sequence(content, MAX_PARTS):
            if isinstance(part, Mapping) and part.get("type") in ("text", "input_text", "output_text"):
                yield part.get("text")
            elif getattr(part, "type", None) in ("input_text", "output_text"):
                yield part.text


def agents_message_text(data, kind, exported):
    """Select message text from generation or response spans."""
    if kind == "generation":
        inputs, outputs = exported.get("input"), exported.get("output")
    elif kind == "response":
        inputs = getattr(data, "input", None)
        outputs = getattr(getattr(data, "response", None), "output", None)
    else:
        return {}
    input_text = join_text([inputs] if isinstance(inputs, str) else _agent_items(inputs))
    output_text = join_text(_agent_items(outputs))
    return {
        f"agenturm.payload.{direction}": text
        for direction, text in (("input", input_text), ("output", output_text))
        if text is not None
    }
