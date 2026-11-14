"""Load only committed synthetic/schema-derived source records into a new DB."""
import json
from pathlib import Path
import sqlite3

ROOT = Path(__file__).parent / 'fixtures' / 'native_review'


def load_current_opencode(path):
    fixture = json.loads((ROOT / 'opencode-2.2.5.synthetic.json').read_text())
    if Path(path).exists():
        raise FileExistsError('Synthetic source destination must be new')
    with sqlite3.connect(path) as db:
        for statement in fixture['schema']:
            db.execute(statement)
        for table, rows in fixture['rows'].items():
            for row in rows:
                fields = list(row)
                values = [json.dumps(value) if isinstance(value, (dict, list)) else value for value in row.values()]
                db.execute('INSERT INTO "' + table + '" (' + ','.join('"' + field + '"' for field in fields) +
                           ') VALUES (' + ','.join('?' for _ in values) + ')', values)
    return fixture
