"""Build a self-contained, responsive HTML companion for repository Markdown.

uv run python scripts/build_readme_html.py
uv run python scripts/build_readme_html.py --markdown docs/product-demo.md --output docs/product-demo.html
Uses markdown-it-py, already provided by Rich. Local images are embedded;
remote badge images become readable labels. All document links stay clickable.
"""
from __future__ import annotations
import argparse
import base64
import html
from html.parser import HTMLParser
import mimetypes
import tomllib
from pathlib import Path
from urllib.parse import urlsplit
from markdown_it import MarkdownIt

ROOT=Path(__file__).resolve().parents[1]
REPO='https://github.com/abhishekash/agenturm/blob/main/'
STYLE='''
:root{color-scheme:light dark;--bg:#f7f5ef;--fg:#222722;--muted:#596157;--line:#ced1c6;--panel:#eeece3;--link:#694799}
@media(prefers-color-scheme:dark){:root{--bg:#100f16;--fg:#f4f1eb;--muted:#b9afc5;--line:#443b52;--panel:#1b1722;--link:#c5adff}}
*{box-sizing:border-box}html{scroll-behavior:smooth}body{margin:0;background:var(--bg);color:var(--fg);font:17px/1.8 system-ui,sans-serif}main{max-width:1040px;margin:auto;padding:32px clamp(20px,5vw,56px) 80px}nav{display:flex;flex-wrap:wrap;gap:12px 24px;border-bottom:1px solid var(--line);padding-bottom:20px;margin-bottom:32px;font-size:14px}a{color:var(--link);text-underline-offset:4px}h1,h2,h3{line-height:1.25;letter-spacing:-.025em}h1{font-size:clamp(32px,6vw,48px)}h2{font-size:30px;margin-top:52px;border-top:1px solid var(--line);padding-top:28px}h3{font-size:23px;margin-top:32px}p,li,td{overflow-wrap:anywhere}p{margin:20px 0}img{display:block;max-width:100%;height:auto;border-radius:6px}p[align=center]{text-align:center}p[align=center] img{margin:auto}pre{background:var(--panel);border:1px solid var(--line);padding:20px;border-radius:6px;overflow:auto;font-size:13px;line-height:1.8}code{font-family:ui-monospace,monospace;font-size:.85em}pre code{font-size:inherit}.table-wrap{overflow:auto;margin:24px 0}table{border-collapse:collapse;width:100%;min-width:480px;font-size:15px}th,td{border-bottom:1px solid var(--line);padding:14px 16px;text-align:left;vertical-align:top}th{background:var(--panel)}td:first-child{min-width:140px}li+li{margin-top:8px}details{padding:20px;border:1px solid var(--line);border-radius:6px}summary{cursor:pointer}.badge{display:inline-block;padding:2px 8px;font-size:12px;background:var(--panel);border:1px solid var(--line);border-radius:4px}hr{border:0;border-top:1px solid var(--line);margin-top:40px}blockquote{border-left:3px solid var(--line);margin:24px 0;padding:0 24px;color:var(--muted)}footer{font-size:13px;color:var(--muted);border-top:1px solid var(--line);padding-top:24px;margin-top:40px}@media(prefers-reduced-motion:reduce){html{scroll-behavior:auto}}@media(max-width:480px){body{font-size:16px}main{padding-inline:20px}h2{font-size:26px}pre{padding:16px;font-size:12px}.table-wrap{margin-inline:-4px}th,td{padding:10px}}
'''

class Embed(HTMLParser):
    def __init__(self,source):super().__init__(convert_charrefs=False);self.source=source;self.parts=[]
    def handle_starttag(self,tag,attributes):
        attrs=dict(attributes)
        if tag=='img':
            src=attrs.get('src','')
            if urlsplit(src).scheme in ('http','https'):
                self.parts.append('<span class="badge">'+html.escape(attrs.get('alt','Badge'))+'</span>');return
            path=(self.source.parent/src).resolve()
            if not path.is_relative_to(ROOT) or not path.is_file():raise ValueError('Missing or outside-repository image: '+src)
            mime=mimetypes.guess_type(path.name)[0] or 'application/octet-stream'
            attrs['src']='data:'+mime+';base64,'+base64.b64encode(path.read_bytes()).decode()
        if tag=='a':
            href=attrs.get('href','')
            if href and not urlsplit(href).scheme and not href.startswith('#'):
                parsed=urlsplit(href);path=(self.source.parent/parsed.path).resolve()
                attrs['href']=REPO+str(path.relative_to(ROOT))+('#'+parsed.fragment if parsed.fragment else '')
        if tag=='table':self.parts.append('<div class="table-wrap" tabindex="0" role="region" aria-label="Scrollable comparison table">')
        self.parts.append('<'+tag+''.join(' '+key+(('="'+html.escape(value,quote=True)+'"') if value is not None else '') for key,value in attrs.items())+'>')
    def handle_endtag(self,tag):
        self.parts.append('</'+tag+'>')
        if tag=='table':self.parts.append('</div>')
    def handle_data(self,data):self.parts.append(data)
    def handle_entityref(self,name):self.parts.append('&'+name+';')
    def handle_charref(self,name):self.parts.append('&#'+name+';')
    def handle_comment(self,data):pass


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--markdown',type=Path,default=ROOT/'README.md')
    parser.add_argument('--output',type=Path,default=ROOT/'docs/readme.html')
    parser.add_argument('--version', help='Override the version shown in the document header')
    args=parser.parse_args();source=args.markdown.resolve()
    project=ROOT/'pyproject.toml'
    version=args.version or (tomllib.loads(project.read_text())['project']['version'] if project.exists() else '')
    product_label='Agenturm'+(' / '+html.escape(version) if version else '')
    rendered=MarkdownIt('commonmark',{'html':True}).enable('table').render(source.read_text())
    embedded=Embed(source);embedded.feed(rendered)
    title='Agenturm — '+('README' if source.name=='README.md' else source.stem.replace('-',' ').title())
    output='<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>'+title+'</title><style>'+STYLE+'</style></head><body><main><nav><strong>'+product_label+'</strong><a href="https://www.agenturm.dev/#demo">Interactive walkthrough ↗</a><a href="'+REPO+str(source.relative_to(ROOT))+'">Markdown source ↗</a></nav>'+''.join(embedded.parts)+'<footer>Self-contained HTML companion. Images and styles are embedded; links open the referenced documentation. Demo records are synthetic.</footer></main></body></html>'
    args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(output)
    print('Wrote',args.output)

if __name__=='__main__':main()
