"""Frozen evidence receipts, with no network or agent execution side effects."""
from __future__ import annotations

from copy import deepcopy
from html import escape
import json
import os
from pathlib import Path
from tempfile import NamedTemporaryFile

from .workflow_evidence import render_review, safe_text


def fenced(text):
    """Keep captured text literal even when it contains Markdown fences."""
    text=safe_text(text)
    fence='```'
    while fence in text:fence+='`'
    return fence+'text\n'+text+'\n'+fence


def receipt_markdown(projection):
    lines=['# Agenturm execution evidence', '',
        'Captured producer records, not independent attestation or verification of the current worktree.', '',
        fenced(render_review(projection)), '', '## Source references', '',
        'Each observation below maps to the selected receiver and its recorded event identity. '
        'References do not guarantee that the receiver will retain those records.', '']
    sources=[]
    for kind in ('request','agent_update'):
        if projection.get(kind):sources.append((kind,projection[kind]))
    for kind in ('checks','edits','errors','context_events'):
        sources.extend((kind,row) for row in projection.get(kind,()))
    for number,(kind,row) in enumerate(sources,1):
        identity={'receiver':projection['scope']['receiver'],**row['source']}
        label=row.get('command') or row.get('path') or row.get('subject') or row.get('text') or ''
        lines += ['### '+str(number)+'. '+kind.replace('_',' '), '',fenced(label), '',
                  fenced(json.dumps(identity,ensure_ascii=False,sort_keys=True,indent=2)), '']
    if not sources:lines+=['No source observations in this loaded window.', '']
    if projection.get('workspace_observation'):
        from .review_continuity import render_workspace
        lines+=['## Frozen local Git observation','',fenced(render_workspace(projection['workspace_observation'])),'']
    if projection.get('parallel_path_observations'):
        from .review_continuity import render_parallel
        lines+=['## Recorded parallel-work observations','',fenced(render_parallel(projection['parallel_path_observations'])),'']
    lines += ['## Frozen scope and capture', '',fenced(json.dumps({
        'format':projection['format'],'scope':projection['scope'],
        'captured_at':projection['captured_at'],'window':projection['window']},ensure_ascii=False,indent=2)), '']
    return '\n'.join(lines)


def document_html(markdown, title='Agenturm execution evidence'):
    from markdown_it import MarkdownIt
    # Captured text stays literal; no images or externally loaded assets.
    body=MarkdownIt('commonmark',{'html':False}).disable('image').render(markdown)
    style='''
    :root {color-scheme:light dark;--bg:#faf9f6;--fg:#22252a;--line:#d4d9df;--panel:#eef0f3;--link:#175bbb}
    @media(prefers-color-scheme:dark){:root{--bg:#10151c;--fg:#e2e7ef;--line:#394552;--panel:#1b2430;--link:#9ac5ff}}
    *{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:17px/1.65 system-ui,sans-serif}
    main{max-width:960px;margin:auto;padding:40px 24px 80px}h1,h2,h3{line-height:1.25;overflow-wrap:anywhere}
    h2{margin-top:2.5rem;border-top:1px solid var(--line);padding-top:1rem}a{color:var(--link)}
    pre{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:16px;white-space:pre-wrap;overflow-wrap:anywhere}
    code{font:14px/1.65 ui-monospace,monospace}table{border-collapse:collapse;width:100%;display:block;overflow:auto}
    th,td{text-align:left;padding:8px;border-bottom:1px solid var(--line)}
    @media(max-width:600px){main{padding:20px 14px 48px}body{font-size:16px}pre{padding:10px}}
    '''
    return '<!doctype html>\n<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>'+escape(title)+'</title><style>'+style+'</style></head><body><main>'+body+'</main></body></html>\n'


def freeze_receipt(projection):
    frozen=deepcopy(projection)
    markdown=receipt_markdown(frozen)
    return {'markdown':markdown,'html':document_html(markdown),
            'json':json.dumps(frozen,ensure_ascii=False,indent=2)+'\n'}


def export_receipt(path, formats):
    """Stage private complete files; refuse overwrites; roll back our links on error.

    Each destination is published atomically. The pair is not crash-atomic.
    """
    target=Path(path).expanduser()
    suffix=target.suffix.lower()
    if suffix not in {'.md','.html','.json'}:raise ValueError('Choose a .md, .html or .json destination; Markdown and HTML companions are always written.')
    contents=[(target.with_suffix('.md'),formats['markdown']),(target.with_suffix('.html'),formats['html'])]
    if suffix=='.json':contents.append((target,formats['json']))
    staged=[];written=[]
    try:
        for destination,content in contents:
            with NamedTemporaryFile(dir=destination.parent,prefix='.agenturm-review-',delete=False) as stream:
                temporary=Path(stream.name);staged.append((temporary,destination))
                if hasattr(os,'fchmod'):os.fchmod(stream.fileno(),0o600)
                stream.write(content.encode('utf-8'));stream.flush();os.fsync(stream.fileno())
        for temporary,destination in staged:
            os.link(temporary,destination);written.append((temporary,destination))
    except BaseException:
        for temporary,destination in written:
            try:
                if os.path.samefile(temporary,destination):destination.unlink()
            except OSError:pass
        raise
    finally:
        for temporary,_ in staged:temporary.unlink(missing_ok=True)
    return [str(destination) for destination,_ in contents]
