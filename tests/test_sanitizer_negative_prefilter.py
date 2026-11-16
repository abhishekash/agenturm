"""Independent exact sanitizer comparisons against frozen main 24ea0a9.

The two small reference modules below were extracted from that commit before
optimization. They are embedded so shallow checkouts and installed-wheel tests
need no Git history, subprocess, network, provider or external fixture.
"""
from copy import deepcopy
import json
import random
import string
import sys
import types
import unittest

from observatory import findings, operation_facts

FROZEN_FINDINGS_SOURCE = r'''import re

_SENSITIVE_FIELD = re.compile(
    r"(?:^|[._-])(?:api[_-]?key|access[_-]?token|refresh[_-]?token|"
    r"client[_-]?secret|[A-Za-z0-9_]+[_-](?:pass|token)(?=$)|password|passwd|private[_-]?key|authorization|"
    r"credential|secret|bearer)(?:$|[._-])", re.IGNORECASE)

_TOKEN_FIELD = re.compile(r"(?:^|[.-])(?:auth[_-]?)?token(?:$|[.-])", re.IGNORECASE)

_ASSIGNED_SECRET = re.compile(
    r"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|"
    r"password|passwd|private[_-]?key|authorization|credential|secret|bearer|"
    r"[A-Za-z][A-Za-z0-9_]*(?:_SECRET_ACCESS_KEY|_API_TOKEN|_PASS|_PASSWORD|_SECRET|_TOKEN))"
    r"(\s*[:=]\s*)([\"']?)([^\s\"'`,;]{1,})")

_DATABASE_CREDENTIAL = re.compile(r"(?i)(\b[a-z][a-z0-9+.-]*://[^\s/@:]*:)[^\s/@]+(@)")

_MYSQL_PASSWORD = re.compile(r"(\bmysql(?:\s+[^\s;]+)*?\s+-p)(?!\s|$)([^\s;]+)")

_BEARER_VALUE = re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._~+/-]{8,}={0,}")

_PRIVATE_KEY = re.compile(
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    re.IGNORECASE)

_TOKEN_PATTERNS = (
    ("Stripe", re.compile(r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b")),
    ("OpenAI or compatible", re.compile(r"\bsk-[A-Za-z0-9_-]{20,}\b")),
    ("GitHub", re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b")),
    ("AWS access key", re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("Google API key", re.compile(r"\bAIza[0-9A-Za-z_-]{30,}\b")),
    ("Slack", re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b")),
)

def _safe_text(value):
    """Remove recognizable credential material before it enters a finding."""
    text = str(value)
    text = _DATABASE_CREDENTIAL.sub(r"\1[REDACTED]\2", text)
    text = _MYSQL_PASSWORD.sub(r"\1[REDACTED]", text)
    text = _PRIVATE_KEY.sub("[REDACTED PRIVATE KEY]", text)
    text = _BEARER_VALUE.sub("Bearer [REDACTED]", text)
    text = _ASSIGNED_SECRET.sub(lambda match: match.group(1) + match.group(2) + "[REDACTED]", text)
    for label, pattern in _TOKEN_PATTERNS:
        text = pattern.sub(f"[REDACTED {label} token]", text)
    return text

def _credential_hits(value, field_path=""):
    """Return safe field/provider labels; never return the matched values."""
    hits = set()
    if isinstance(value, dict):
        for key, child in value.items():
            key = str(key)
            path = f"{field_path}.{key}" if field_path else key
            if (_SENSITIVE_FIELD.search(key) or _TOKEN_FIELD.search(key)) and child not in (None, "", False):
                hits.add(("sensitive field", path[:160]))
            hits.update(_credential_hits(child, path))
    elif isinstance(value, (list, tuple)):
        for child in value[:100]:
            hits.update(_credential_hits(child, field_path))
    elif isinstance(value, str):
        if _PRIVATE_KEY.search(value):
            hits.add(("private key material", field_path[:160] or "captured text"))
        if _ASSIGNED_SECRET.search(value):
            hits.add(("credential assignment", field_path[:160] or "captured text"))
        if _BEARER_VALUE.search(value):
            hits.add(("bearer token", field_path[:160] or "captured text"))
        for label, pattern in _TOKEN_PATTERNS:
            if pattern.search(value):
                hits.add((label + " token pattern", field_path[:160] or "captured text"))
    return hits
'''

FROZEN_OPERATION_SOURCE = r'''import json, math, re, unicodedata

MAX_TEXT=8000

MAX_NODES=128

MAX_DEPTH=6

QUOTED_FIELDS=(re.compile(r'"((?:\\.|[^"\\\r\n])*)"\s*:'),
               re.compile(r"'((?:\\.|[^'\\\r\n])*)'\s*:"))

_ASCII_KEY_CONTROLS=re.compile(r'[\x00-\x1f\x7f]')

_ASCII_TEXT_CONTROLS=re.compile(r'[\x00-\x08\x0b-\x1f\x7f]')

_PLAIN_SCHEMA_WORDS=frozenset(('command','arguments','cwd','tool','server','domain','action',
    'inputs','commands','exits','outputs'))

def credential_key(key):
    from .findings import _SENSITIVE_FIELD,_TOKEN_FIELD
    if len(key)>MAX_TEXT:return True
    if type(key)is str and key in _PLAIN_SCHEMA_WORDS:return False
    if '\\' in key:
        try:key=json.loads('"'+key+'"')
        except (ValueError,TypeError):return True
        if '\\' in key:return True
    key=(_ASCII_KEY_CONTROLS.sub('',key) if key.isascii() else
        ''.join(char for char in key if unicodedata.category(char) not in {'Cc','Cf'}))
    return bool(_SENSITIVE_FIELD.search(key) or _TOKEN_FIELD.search(key))

def safe_value(value):
    """Return bounded redacted JSON and whether exact matching remains safe."""
    from .findings import _safe_text
    nodes=MAX_NODES;bytes_left=MAX_TEXT;complete=True
    def visit(item,depth=0):
        nonlocal nodes,bytes_left,complete
        nodes-=1
        if nodes<0 or depth>MAX_DEPTH:
            complete=False;return '[payload omitted]'
        if isinstance(item,str):
            if len(item)>bytes_left:
                complete=False;return '[text omitted: projection limit]'
            plain_word=type(item)is str and item in _PLAIN_SCHEMA_WORDS
            try:size=len(item)if plain_word else len(item.encode('utf-8'))
            except UnicodeEncodeError:
                complete=False;return '[invalid Unicode omitted]'
            if size>bytes_left:
                complete=False;return '[text omitted: projection byte limit]'
            bytes_left-=size
            if plain_word:return item
            if any(credential_key(match.group(1)) for pattern in QUOTED_FIELDS for match in pattern.finditer(item)):
                complete=False;return '[REDACTED credential-bearing text]'
            text=_safe_text(item)
            text=re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',text)
            text=(_ASCII_TEXT_CONTROLS.sub('',text) if text.isascii() else
                ''.join(char for char in text if char in '\n\t' or unicodedata.category(char) not in {'Cc','Cf'}))
            if text!=item or re.search(r'(?i)\[(?:redact|[^\]]*(?:omitted|truncated|clipped))',text):complete=False
            return text
        if item is None or type(item) is bool:return item
        if type(item) is int and item.bit_length()<=4096:return item
        if type(item) is float and math.isfinite(item):return item
        if isinstance(item,dict):
            result={}
            for key,child in item.items():
                if nodes<=0:complete=False;break
                if not isinstance(key,str):complete=False;continue
                clean_key=visit(key,depth+1)
                if credential_key(key) or credential_key(clean_key):
                    complete=False;result[clean_key]='[REDACTED]'
                else:result[clean_key]=visit(child,depth+1)
            return result
        if isinstance(item,(list,tuple)):
            result=[]
            for child in item:
                if nodes<=0:complete=False;break
                result.append(visit(child,depth+1))
            return result
        complete=False;return '[unsupported value]'
    return visit(value),complete
'''

class SanitizerNegativePrefilterTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.prefix = '_agenturm_frozen_sanitizer_reference'
        package = types.ModuleType(cls.prefix)
        package.__path__ = []
        package.__package__ = cls.prefix
        sys.modules[cls.prefix] = package
        for name, source in (('findings', FROZEN_FINDINGS_SOURCE),
                             ('operation_facts', FROZEN_OPERATION_SOURCE)):
            module = types.ModuleType(cls.prefix + '.' + name)
            module.__package__ = cls.prefix
            sys.modules[module.__name__] = module
            exec(compile(source, '<frozen-24ea0a9-' + name + '>', 'exec'), module.__dict__)
            setattr(package, name, module)
        cls.legacy_findings = package.findings
        cls.legacy_operation = package.operation_facts

    @classmethod
    def tearDownClass(cls):
        for name in ('findings', 'operation_facts'):
            sys.modules.pop(cls.prefix + '.' + name, None)
        sys.modules.pop(cls.prefix, None)

    def assert_text(self, value):
        self.assertEqual(findings._safe_text(value), self.legacy_findings._safe_text(value))
        self.assertEqual(findings._credential_hits(value), self.legacy_findings._credential_hits(value))

    def assert_value(self, value):
        expected = self.legacy_operation.safe_value(deepcopy(value))
        actual = operation_facts.safe_value(deepcopy(value))
        self.assertEqual(actual, expected)
        self.assertEqual(findings._credential_hits(deepcopy(value), 'generated.root'),
                         self.legacy_findings._credential_hits(deepcopy(value), 'generated.root'))

    def test_every_secret_pattern_and_prefix_keeps_exact_redaction_and_labels(self):
        values = [
            'password=GENERATED_VALUE', 'API_KEY: "GENERATED_VALUE"',
            'ACCESS_TOKEN=GENERATED_VALUE', 'refresh-token: GENERATED_VALUE',
            'CLIENT_SECRET=GENERATED_VALUE', 'AWS_SECRET_ACCESS_KEY=GENERATED_VALUE',
            'SOME_API_TOKEN=GENERATED_VALUE', 'DB_PASS=GENERATED_VALUE',
            'postgresql+psycopg://generated:GENERATED_VALUE@example.invalid/db',
            'mysql --host example.invalid -pGENERATED_VALUE',
            'mysql\n--port 3306 -pGENERATED_VALUE',
            'Bearer ' + 'A' * 24, 'bEaReR ' + 'Z' * 24,
            '-----BEGIN OPENSSH PRIVATE KEY-----\nGENERATED\n-----END OPENSSH PRIVATE KEY-----',
            'sk_live_' + 'A' * 16, 'rk_test_' + 'B' * 16,
            'sk-' + 'A' * 20, 'github_pat_' + 'A' * 20,
            'AKIA' + 'A' * 16, 'AIza' + 'B' * 30,
        ]
        values += ['gh' + role + '_' + 'C' * 20 for role in 'pousr']
        values += ['xox' + role + '-' + 'D' * 20 for role in 'baprs']
        for value in values:
            with self.subTest(pattern=value[:32]):
                self.assertNotEqual(self.legacy_findings._safe_text(value), value)
                self.assert_text(value)
                self.assert_value({'payload': value})
                for wrapped in ('(' + value + ')', 'prefix ' + value + ' suffix', value.lower(), value.upper()):
                    self.assert_text(wrapped)

    def test_sensitive_field_boundaries_escapes_and_control_removal_are_unchanged(self):
        keys = [
            'api_key', 'api-key', 'apiKey', 'access_token', 'refresh-token',
            'client_secret', 'database_password', 'passwd', 'private_key',
            'authorization', 'credential', 'secret', 'bearer', 'token', 'auth-token',
            'NODE_TOKEN', 'DB_PASS', 'AWS_SECRET_ACCESS_KEY',
            'api\\u005fkey', 'pass\\u0077ord', 'api_\x00key', 'api_\x7fkey',
            'api_\u200bkey', 'pass\u2060word', 'api_\\qkey',
            'ordinary', 'command', 'arguments', 'cwd', 'tool', 'server',
            'domain', 'action', 'inputs', 'outputs', 'keyframe', 'tokenizer',
            'passwordish', 'monkey', 'prefix_token_suffix', 'x' * 8001,
        ]
        for key in keys:
            with self.subTest(key=repr(key[:40])):
                self.assertEqual(operation_facts.credential_key(key), self.legacy_operation.credential_key(key))
                for child in ('GENERATED_VALUE', '', None, False, 0, True, {'payload': 'Bearer ' + 'A' * 20}):
                    self.assert_value({key: child})

    def test_unicode_casefold_regex_equivalences_always_match_frozen_behavior(self):
        values = [
            'api_Key=GENERATED_VALUE', 'paſſword=GENERATED_VALUE',
            'prİvate_key=GENERATED_VALUE', 'authorization: GÉNÉRATED',
            'ordinary café', '日本語の入力', 'Grüße', 'π = 3.14',
            'safe\u200btext', 'api_\u200bkey=GENERATED_VALUE',
            '\ud800', '\udfff', 'x\ud800y',
        ]
        for value in values:
            with self.subTest(value=repr(value)):
                self.assert_text(value)
                self.assert_value({'payload': value})
                self.assert_value({value: 'GENERATED_VALUE'})

    def test_quoted_fields_ansi_controls_omission_markers_and_budgets_are_exact(self):
        values = [
            '{"api_key":"GENERATED_VALUE"}', "{'password': 'GENERATED_VALUE'}",
            '{"api\\u005fkey":"GENERATED_VALUE"}',
            '{"safe":"password=GENERATED_VALUE"}',
            '"safe": ordinary', "'safe': ordinary", 'no quoted field here',
            '\x1b[31mordinary\x1b[0m', '\x1b]0;generated\x07ordinary',
            'line\nwith\ttabs', 'carriage\rreturn', 'null\x00byte', '\x7fdelete',
            '[REDACTED]', '[redact]', '[output omitted]', '[truncated]',
            '[clipped output]', '[safe bracket]', 'ordinary omitted words',
            'x' * 8000, 'x' * 8001, 'é' * 4000, 'é' * 4001,
        ]
        for value in values:
            with self.subTest(value=repr(value[:40])):
                self.assert_text(value)
                self.assert_value(value)
        self.assert_value([chr(number) for number in range(128)])
        self.assert_value({'payload': ''.join(chr(number) for number in range(128))})

    def test_nested_aliases_depth_nodes_nonstring_keys_and_scalar_limits_are_exact(self):
        shared = {'safe': 'ordinary', 'TOKEN': 'GENERATED_VALUE'}
        values = [
            {'left': shared, 'right': shared},
            [{'level': [{'payload': shared}]}],
            tuple(range(150)), [shared] * 110,
            {0: 'ignored', None: 'ignored', 'safe': True},
            {'number': 2 ** 4097, 'nan': float('nan'), 'inf': float('inf')},
            {'negative': -1, 'float': 0.25, 'null': None},
        ]
        deep = 'GENERATED_VALUE'
        for _ in range(10):
            deep = {'payload': [deep]}
        values.append(deep)
        for value in values:
            with self.subTest(shape=type(value).__name__):
                self.assert_value(value)

    def test_seeded_adversarial_combinations_preserve_full_values_and_completeness(self):
        rng = random.Random(241115)
        alphabet = string.ascii_letters + string.digits + " ._-=:/@'\\\"[]\n\t\x00\x1b" + 'éKſİ\u200b'
        seeds = ['ordinary', 'password=GENERATED_VALUE', 'TOKEN', 'AKIA' + 'A' * 16,
                 'AIza' + 'B' * 30, 'sk-' + 'Z' * 20, '[clipped]',
                 '"api\\u005fkey": "GENERATED_VALUE"', 'Bearer ' + 'A' * 20]
        for number in range(500):
            padding = ''.join(rng.choice(alphabet) for _ in range(rng.randrange(0, 60)))
            value = padding + rng.choice(seeds) + padding[::-1]
            with self.subTest(case=number):
                self.assert_text(value)
                self.assert_value({'safe': [value], rng.choice(seeds): value})

    def test_string_subclasses_cannot_lie_to_ascii_negative_prefilter(self):
        class MisleadingString(str):
            def isascii(self):
                return True
            def lower(self):
                return 'ordinary'
            def casefold(self):
                return 'ordinary'
        for raw in ('paſſword=GENERATED_VALUE', 'Bearer ' + 'A' * 20,
                    'AKIA' + 'A' * 16, 'api_key'):
            value = MisleadingString(raw)
            self.assert_text(value)
            self.assert_value({'payload': value})
            self.assert_value({value: 'GENERATED_VALUE'})


if __name__ == '__main__':
    unittest.main()
