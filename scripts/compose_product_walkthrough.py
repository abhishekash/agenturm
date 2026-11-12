"""Compose a 37-second silent, captioned tour from actual v0.2.0 screen captures.

uv run --no-project --with imageio-ffmpeg --with pillow python scripts/compose_product_walkthrough.py
Run capture_product_walkthrough.py first. No user records or network are read.
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
from imageio_ffmpeg import write_frames

ROOT=Path(__file__).resolve().parents[1]
SCENES=[
    ('agents',6,'01 / AGENTS','See the task and its workers.','Each agent keeps its own state. A branch can carry a request for you.'),
    ('prompts',5,'02 / PROMPTS','Recover the original request.','Choose a request with the keyboard; read the captured text in its own space.'),
    ('prompt-constraints',3,'02 / PROMPTS','Keep the constraints in view.','Read the next request without losing the session’s context.'),
    ('prompt-review',3,'02 / PROMPTS','Read the later steering.','The developer asked for tests and visibility into changes after them.'),
    ('review',8,'03 / REVIEW','Inspect what the check establishes.','The captured pytest command passed. Later recorded edits require another check.'),
    ('mcp',6,'04 / TOOLS','Read the response behind a failure.','The GitHub MCP call returned HTTP 403: API rate limit exceeded.'),
    ('handoff',6,'05 / HANDOFF','Carry the decision and next action.','Review a saved handoff, then copy it into the agent you choose.'),
]
SIZE=(1600,1160);FPS=12
FONT=Path('/usr/share/fonts/truetype/dejavu')
def font(size,bold=False,mono=False):
    return ImageFont.truetype(str(FONT/(('DejaVuSansMono' if mono else 'DejaVuSans')+('-Bold' if bold else '')+'.ttf')),size)

def frame(name,chapter,title,note):
    canvas=Image.new('RGB',SIZE,'#100f16');draw=ImageDraw.Draw(canvas)
    draw.text((36,20),'agenturm / 0.2.0',font=font(20,True),fill='#f4f1eb')
    draw.text((SIZE[0]-230,23),chapter,font=font(15,mono=True),fill='#ffbd69')
    image=Image.open(ROOT/'media/product'/f'{name}.png').convert('RGB')
    image.thumbnail((SIZE[0]-40,920),Image.Resampling.LANCZOS)
    canvas.paste(image,((SIZE[0]-image.width)//2,66+(920-image.height)//2))
    draw.text((36,1005),title,font=font(31,True),fill='#f4f1eb')
    draw.text((36,1055),note,font=font(18),fill='#b9afc5')
    draw.text((36,1115),'ACTUAL TERMINAL UI / SYNTHETIC RECORDS / SILENT WALKTHROUGH',font=font(12,mono=True),fill='#b9afc5')
    return canvas

def main():
    target=ROOT/'media/product/walkthrough.mp4'
    writer=write_frames(str(target),SIZE,fps=FPS,codec='libx264',pix_fmt_in='rgb24',pix_fmt_out='yuv420p',quality=7,macro_block_size=2,output_params=['-movflags','+faststart'])
    writer.send(None)
    gif=[];duration=[]
    try:
        for name,seconds,chapter,title,note in SCENES:
            image=frame(name,chapter,title,note)
            if not gif:image.save(ROOT/'media/product/walkthrough-poster.png',optimize=True)
            for _ in range(seconds*FPS):writer.send(image.tobytes())
            preview=image.copy();preview.thumbnail((1000,1000),Image.Resampling.LANCZOS);gif.append(preview);duration.append(seconds*1000)
    finally:writer.close()
    gif[0].save(ROOT/'media/product/walkthrough.gif',save_all=True,append_images=gif[1:],duration=duration,loop=0,optimize=True)
    transcript='Agenturm 0.2.0 — 37-second silent terminal walkthrough\nActual terminal UI; synthetic example records.\n\n'+'\n\n'.join(chapter+' / '+title+'\n'+note for _,_,chapter,title,note in SCENES)+'\n\nThe test worker’s passed check precedes later edits. No fresh validation is established.\nPrompts, decisions, and all task identities are fictional. Agenturm does not send the handoff or launch an agent.\n'
    (ROOT/'media/product/walkthrough.txt').write_text(transcript)
    print('Wrote',target,'37 seconds; actual screen states, with explanatory captions.')

if __name__=='__main__':main()
