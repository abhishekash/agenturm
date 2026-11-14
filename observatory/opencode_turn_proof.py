"""Validate bounded current-runtime turn membership without borrowing session usage."""
from __future__ import annotations
import hashlib
import json
import re

PREFIX='observatory.opencode.turn.'
FIELDS=('schema','source_epoch','boundary_kind','boundary_id','boundary_seq','closed',
        'closed_id','closed_seq','expected_assistants','membership_sha256','source_complete')


def membership_digest(trace_ids):
    return hashlib.sha256(json.dumps(sorted(set(trace_ids)),ensure_ascii=True,
        separators=(',',':')).encode('ascii')).hexdigest()


def turn_coverage(selected, members, trace_ids, *, clipped=False):
    """Legacy parent contracts remain separate; schema2 requires every proof field.

    Unknowns are generic reasons, never untrusted native strings. Each returned
    member is a retained assistant trace, not a guessed session-wide interval.
    """
    if not any(key.startswith(PREFIX) for key in selected):
        if any(any(key.startswith(PREFIX) for key in item) for item in members):
            return ['Turn membership mixes legacy and current source contracts']
        return []
    proof={key:selected.get(PREFIX+key) for key in FIELDS}
    reasons=[]
    if type(proof['schema']) is not int or proof['schema']!=2:
        reasons.append('Turn membership schema is unsupported')
    if not isinstance(proof['source_epoch'],str) or not re.fullmatch('[0-9a-f]{32}',proof['source_epoch']):
        reasons.append('Turn source generation is unavailable')
    if proof['boundary_kind']!='idle' or not isinstance(proof['boundary_id'],str) or not proof['boundary_id'] or len(proof['boundary_id'])>1024 or type(proof['boundary_seq']) is not int or proof['boundary_seq']<0:
        reasons.append('A recorded preceding idle boundary is unavailable')
    if type(proof['closed']) is not bool:
        reasons.append('Turn closure evidence is unavailable')
    elif proof['closed']:
        if not isinstance(proof['closed_id'],str) or not proof['closed_id'] or len(proof['closed_id'])>1024 or type(proof['closed_seq']) is not int or type(proof['boundary_seq']) is not int or proof['closed_seq']<=proof['boundary_seq']:
            reasons.append('The recorded closing idle boundary is incomplete')
    elif proof['closed_id'] not in (None,'') or proof['closed_seq'] is not None:
        reasons.append('Open turn has contradictory closure evidence')
    if type(proof['source_complete']) is not bool or not proof['source_complete']:
        reasons.append('Native turn source membership is partial')
    if clipped:reasons.append('Turn membership exceeds the retained member limit')
    unique=set(trace_ids)
    if len(unique)!=len(trace_ids):reasons.append('Turn assistant roots are ambiguous')
    if type(proof['expected_assistants']) is not int or proof['expected_assistants']<1 or proof['expected_assistants']>1000 or proof['expected_assistants']!=len(unique):
        reasons.append('Expected turn assistant membership is not fully indexed')
    digest=proof['membership_sha256']
    if not isinstance(digest,str) or not re.fullmatch('[0-9a-f]{64}',digest) or digest!=membership_digest(trace_ids):
        reasons.append('Recorded turn membership differs from retained assistant steps')
    if any({key:item.get(PREFIX+key) for key in FIELDS}!=proof for item in members):
        reasons.append('Assistant steps disagree about their native turn boundary or membership')
    return reasons
