"""Keep the manual real-session witness strict across responsive roster layouts."""

import importlib.util
from pathlib import Path
import sys
import unittest

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
spec = importlib.util.spec_from_file_location("real_roster_witness", SCRIPTS / "validate_rust_real_session_roster.py")
roster = importlib.util.module_from_spec(spec)
sys.path.insert(0, str(SCRIPTS))
sys.modules[spec.name] = roster
try:
    spec.loader.exec_module(roster)
finally:
    sys.path.pop(0)


def frame(*, provider=True, updated=True, state="Ready", query="Release checks", context=None):
    columns = [(" Task", 35)]
    values = [("›   Release checks", 35)]
    if provider:
        columns.append(("Agent", 12))
        values.append(("Codex", 12))
    columns.append(("Status", 11))
    values.append((state, 11))
    if updated:
        columns.append(("Updated↓", 9))
        values.append(("2m ago", 9))
    header = "│" + "".join(f"{text:<{width}}" for text, width in columns) + "│"
    selected = "│" + "".join(f"{text:<{width}}" for text, width in values) + "│"
    extra = [] if context is None else ["│" + context.ljust(len(header) - 2) + "│"]
    return "\n".join((
        "Agenturm command center",
        header,
        selected,
        *extra,
        "│    Different task                 Working    3m ago   │",
        f"╰ / {query} ──────────────────────────────────────────╯",
        "Enter inspect   / edit search   a reset   ? help",
    ))


class RealRosterWitnessTests(unittest.TestCase):
    def test_committed_search_uses_contour_and_exact_query(self):
        screen = frame()
        self.assertIsNotNone(roster.roster_selection(screen, "Release checks"))
        for query in ("Release", "Release checks extra", ""):
            self.assertIsNone(roster.roster_selection(screen, query))
        self.assertIsNone(roster.roster_selection(screen.replace("Release checks ─", "Release checks▏ ─"), "Release checks"))
        self.assertIsNone(roster.roster_selection(screen + "\nBackspace edit", "Release checks"))

    def test_status_follows_rendered_header_with_responsive_columns(self):
        for provider, updated in ((True, True), (True, False), (False, True)):
            for state in ("Ready", "Working", "Needs you", "Unknown"):
                with self.subTest(provider=provider, updated=updated, state=state):
                    screen = frame(provider=provider, updated=updated, state=state)
                    self.assertEqual(roster.roster_selected_status(screen), state)

    def test_one_line_task_does_not_borrow_next_task_as_context(self):
        selected, context = roster.roster_selection(frame(), "Release checks")
        self.assertIn("Release checks", selected)
        self.assertEqual(context, "")
        self.assertNotIn("Different task", context)

    def test_supporting_context_is_optional_and_stays_inside_roster(self):
        selected, context = roster.roster_selection(frame(context="    Mira · reviewer"), "Release checks")
        self.assertEqual(context, "Mira · reviewer")
        wide = frame()
        wide = "\n".join(line + " │ › preview metadata │" if line.startswith("│") else line for line in wide.splitlines())
        self.assertIsNotNone(roster.roster_selection(wide, "Release checks"))
        self.assertEqual(roster.roster_selected_status(wide), "Ready")
        self.assertNotIn("preview", roster.roster_selection(wide, "Release checks")[0])

    def test_ambiguous_selection_and_missing_header_cannot_prove_status(self):
        screen = frame()
        selected = next(line for line in screen.splitlines() if line.startswith("│›"))
        ambiguous = screen + "\n" + selected
        self.assertIsNone(roster.roster_selection(ambiguous, "Release checks"))
        self.assertIsNone(roster.roster_selected_status(ambiguous))
        self.assertIsNone(roster.roster_selected_status(screen.replace("Status", "State")))

    def test_inspector_owner_remains_exact_source_and_session(self):
        source = "harness.codex.source-a"
        owner = f"│Source  {source} │\n│Session  same-native-id │"
        self.assertEqual(roster.inspector_owner(owner), (source, "same-native-id"))
        self.assertNotEqual(roster.inspector_owner(owner), ("harness.codex.source-b", "same-native-id"))


if __name__ == "__main__":
    unittest.main()
