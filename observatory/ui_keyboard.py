"""A persistent, keyboard-readable reference for the existing navigation paths."""
import re
from rich.text import Text
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import RichLog, Static


KEYBOARD_REFERENCE = '''MOVE AND READ
Tab / Shift+Tab   Next / previous visible control
↑ / ↓            Move in a list; scroll when the text reader has focus
PgUp / PgDn      Move a page in the focused list or reader
Enter / Space    Activate a focused button or open a dropdown
Esc              Close a dropdown first; otherwise return one screen
F1               Open this reference from any screen; F1 or Esc returns
?                Open help in the monitor (text fields keep literal ?)

AGENTS AND INSPECTOR
↑ / ↓            Choose an agent
→ / ←            Expand / collapse its recorded subagent branch
Enter / e        Open the inspector
← / →            Choose Overview, Prompts or Details on the tab strip
Enter / ↓        Enter the chosen section
Ctrl+← / Ctrl+→  Change inspector section from its content
m                Next inspector section
Ctrl+R           Review the selected agent's recorded work
Esc              Return to the selected agent

PROMPTS
Entering Prompts focuses its request navigator, starting at the latest.
Wide index: ↑ / ↓ chooses a request; Home / End chooses oldest / latest.
Wide index: Enter or → reads the selected request.
Compact dropdown: ↑ / ↓, Enter or Space opens the choices.
Open dropdown: ↑ / ↓ or Home / End chooses; Enter confirms and reads.
Compact dropdown: → reads the current request without opening the choices.
Text reader: ↑ / ↓, PgUp / PgDn and Home / End scroll that request.
Text reader: ← returns to the request navigator.
[ / ]            Previous / next loaded request
l                Latest loaded request
z                Toggle readable / original captured text
c / x            Copy / preview an export of the selected request
Tab              Reach Earlier prompts or Retry prompts when available
New arrivals and resize preserve the selected request and reading position.

OTHER MONITOR VIEWS
1–7              Agents, Activity, MCP, Traces, Host, Metrics, Findings
/                Focus search (Esc returns to the list)
↑ / ↓, Enter     Choose a result, then inspect it
e / Esc          Expand an inspector / return to the list
[ / ] / l        Newer page / older page / latest page
c / x            Copy / preview export of selected evidence
o                Agents sort options
f                Agents tree / flat list
p                Pause / resume display updates
s / t            Open related activity / trace where available
Alt+←            Return to the previous evidence location
Alt+↑ / Alt+↓    Agent parent / first child
Alt+→            Next descendant needing attention
Ctrl+P           Command palette
q                Quit from the monitor; text fields keep ordinary typing

WORKFLOW
w                Open from the monitor
↑ / ↓            Choose an attention incident immediately on entry
Enter            Read it (narrow layouts use a separate detail view)
Tab / Enter      Reach and activate Review, Seen, Retry or Settings
Esc              Close settings, return to queue, then return to monitor
Seen records a local acknowledgement; it never approves a native request.

REVIEW
↑ / ↓, PgUp/Dn   Scroll recorded evidence immediately on entry
R                Refresh the frozen review
c / x            Copy / preview export
Tab / Enter      Reach earlier evidence, Save task or Excerpt
Esc              Return to the previous screen

SAVED TASKS
b                Open from the monitor
↑ / ↓, Enter     Choose and open a saved task
n                New task (in the task list) / new decision (in Decisions)
Task detail: ↑ / ↓ and PgUp / PgDn scroll immediately on entry.
p / d            Open the task's Brief / Decisions
e / l / h        Edit / link selected run / prepare handoff
r / Ctrl+R       Capture workspace / reload saved task
Decisions: Enter opens versions; arrows choose; Enter confirms.
Tab              Reach decision text, Runs, Archive and other actions
Editors: Tab / Shift+Tab moves fields; Ctrl+S saves.
Editors: Esc closes a dropdown first. Unsaved edits need a second Esc.
Handoff: PgUp / PgDn reads; Tab reaches copy, destination and write.
Esc              Return one level without changing the selected monitor agent

COPY AND TEXT INPUT
Ctrl+C           Copy selected text or a text field's selection
Ordinary letters, brackets and numbers stay text while editing/searching.
F1 remains available while editing; closing help preserves the draft.
'''


class KeyboardHelpScreen(ModalScreen):
    DEFAULT_CSS = '''
    KeyboardHelpScreen { background: $surface; padding: 0 1; }
    #keyboard-heading { height: auto; max-height: 2; text-style: bold; }
    #keyboard-reference { height: 1fr; }
    #keyboard-hints { height: 1; }
    '''
    BINDINGS=[Binding('escape','dismiss','Back')]

    def __init__(self,context):
        super().__init__();self.context=context

    def compose(self):
        with Vertical():
            yield Static('Keyboard navigation · '+self.context,id='keyboard-heading',markup=False)
            yield RichLog(id='keyboard-reference',wrap=True,min_width=1,markup=False,auto_scroll=False)
            yield Static('↑↓ / PgUp/PgDn Read · F1 / Esc Return',id='keyboard-hints',markup=False)

    def on_mount(self):
        sections=re.split(r'\n\n(?=[A-Z][A-Z /]+\n)',KEYBOARD_REFERENCE)
        context=self.context.lower()
        title=('PROMPTS' if 'prompts' in context else 'WORKFLOW' if 'workflow' in context else
               'REVIEW' if 'review' in context else 'SAVED TASKS' if 'task' in context or 'handoff' in context else
               'AGENTS AND INSPECTOR' if 'agents' in context else 'OTHER MONITOR VIEWS')
        sections.sort(key=lambda section:0 if section.startswith(title+'\n') else 1)
        text=Text('\n\n'.join(sections))
        text.highlight_regex(r'(?m)^[A-Z][A-Z /]+$',style='bold')
        log=self.query_one(RichLog);log.write(text);log.focus()
