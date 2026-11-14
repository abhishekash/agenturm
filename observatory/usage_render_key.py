"""Bounded immutable inputs for one app-local pure usage-formatting guard."""
import math

MAX_NODES = 2048
MAX_TEXT_BYTES = 32768
MAX_DEPTH = 16


def usage_render_key(*values):
    """Detach JSON-shaped values, or decline caching without keeping references.

    Type tags distinguish booleans, integers and floats. Unsupported objects,
    cycles and oversized values use the ordinary formatting path each time.
    """
    nodes = MAX_NODES
    text_bytes = MAX_TEXT_BYTES
    active = set()

    def freeze(value, depth=0):
        nonlocal nodes, text_bytes
        nodes -= 1
        if nodes < 0 or depth > MAX_DEPTH:
            raise ValueError('Usage key budget')
        kind = type(value)
        if value is None:
            return ('null',)
        if kind is bool:
            return ('bool', value)
        if kind is int:
            if value.bit_length() > 128:
                raise ValueError('Usage key integer budget')
            return ('int', value)
        if kind is float:
            if not math.isfinite(value):
                raise ValueError('Nonfinite usage key')
            return ('float', value.hex())
        if kind is str:
            if len(value) > text_bytes:
                raise ValueError('Usage key text budget')
            text_bytes -= len(value.encode('utf-8'))
            if text_bytes < 0:
                raise ValueError('Usage key text budget')
            return ('str', value)
        if kind not in (dict, list, tuple) or id(value) in active:
            raise ValueError('Unsupported or cyclic usage key')
        if len(value) * (2 if kind is dict else 1) > nodes:
            raise ValueError('Usage key node budget')
        active.add(id(value))
        try:
            if kind is dict:
                if any(type(key) is not str for key in value):
                    raise ValueError('Usage key object keys must be strings')
                return ('dict', tuple((freeze(key, depth+1), freeze(value[key], depth+1))
                                      for key in sorted(value)))
            return ('list' if kind is list else 'tuple',
                    tuple(freeze(child, depth+1) for child in value))
        finally:
            active.remove(id(value))

    try:
        return freeze(values)
    except (ValueError, RuntimeError, UnicodeError):
        return None
