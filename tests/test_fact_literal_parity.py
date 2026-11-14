"""Finite literal recognition preserves all surrounding sanitizer budgets."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import unittest
from unittest.mock import patch

from observatory import operation_facts as facts
from observatory import findings

WORDS=('command','arguments','cwd','tool','server','domain','action','inputs','commands','exits','outputs')


def examples():
    for word in WORDS:
        yield word+':plain',word
        yield word+':secret-value',{word:'Bearer SYNTHETIC_PRIVATE_1234567890'}
        yield word+':secret-parent',{'password':word}
        yield word+':quoted',json.dumps({'password':word})
        for length in (7980,7990,7995,7999,8000,8001):yield word+':budget:'+str(length),['x'*length,word]
        for depth in (5,6,7):
            value=word
            for _ in range(depth):value=[value]
            yield word+':depth:'+str(depth),value
        for count in (126,127,128):yield word+':nodes:'+str(count),[word]*count


def output_hash(value):
    return hashlib.sha256(json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()).hexdigest()


class FactLiteralParityTests(unittest.TestCase):
    def test_current_redaction_rules_prove_each_literal_is_benign(self):
        self.assertEqual(facts._PLAIN_SCHEMA_WORDS,frozenset(WORDS))
        for word in WORDS:
            self.assertEqual(findings._safe_text(word),word)
            self.assertIsNone(findings._SENSITIVE_FIELD.search(word))
            self.assertIsNone(findings._TOKEN_FIELD.search(word))
            self.assertEqual(facts.safe_value(word),(word,True))

    def test_full_outputs_match_frozen_e795_budget_and_secret_cases(self):
        frozen=json.loads((Path(__file__).parent/'fixtures/fact-literal-e795.json').read_text())
        observed={label:output_hash(facts.safe_value(deepcopy(value)))for label,value in examples()}
        self.assertEqual(observed,frozen['output_hashes'])

    def test_small_limits_and_string_subclasses_never_bypass_checks(self):
        with patch.object(facts,'MAX_TEXT',3):
            self.assertTrue(facts.credential_key('command'))
            self.assertEqual(facts.safe_value('command'),('[text omitted: projection limit]',False))
        with patch.object(facts,'MAX_NODES',0):
            self.assertEqual(facts.safe_value('command'),('[payload omitted]',False))
        with patch.object(facts,'MAX_DEPTH',0):
            self.assertEqual(facts.safe_value(['command']),(['[payload omitted]'],False))
        class InvalidUnicode(str):
            def encode(self,*args,**kwargs):raise UnicodeEncodeError('utf-8',self,0,1,'synthetic')
        self.assertEqual(facts.safe_value(InvalidUnicode('command')),('[invalid Unicode omitted]',False))
        value,complete=facts.safe_value({'access_token':'command','nested':{'password':'server'}})
        self.assertEqual(value,{'access_token':'[REDACTED]','nested':{'password':'[REDACTED]'}})
        self.assertFalse(complete)
        self.assertTrue(facts.credential_key(r'api\u005fkey'))
