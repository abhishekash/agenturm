"""Keep native bundle qualification strict across responsive roster layouts."""

import importlib.util
from pathlib import Path
import sys
import unittest


SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
spec = importlib.util.spec_from_file_location(
    "default_cockpit_validation", SCRIPTS / "validate_rust_default_cockpit.py"
)
cockpit = importlib.util.module_from_spec(spec)
sys.path.insert(0, str(SCRIPTS))
try:
    spec.loader.exec_module(cockpit)
finally:
    sys.path.pop(0)


SESSION = "bundle-pi-session"
CONTOURED = "\n".join((
    " Agenturm command center                   F2 Themes  ? Help",
    "╭ Tasks ───────────────────────────────────────────────────╮",
    "│   Task                              Agent      Status    │",
    "│   No recorded project · 1                                │",
    "│›   Pi session                       Pi         Ready     │",
    "╰ / bundle-pi-session ──────────────────────────────────────╯",
    "Collection complete · ? details",
    "Enter inspect   / edit search   a reset   ? help",
))
LEGACY = "\n".join((
    " Agenturm command center",
    f"  {'Tasks':<34} {'Status':<10} Updated↓",
    "   No recorded project 1",
    f"› {'Pi session':<34} {'Ready':<10} 12h ago",
    " / bundle-pi-session",
    "Enter / Esc search · a reset · q quit",
))


class RustDefaultCockpitValidationTests(unittest.TestCase):
    def test_committed_search_is_exact_in_legacy_and_contoured_rosters(self):
        for frame in (LEGACY, CONTOURED):
            with self.subTest(frame=frame):
                self.assertEqual(cockpit.roster_search(frame), SESSION)
                self.assertTrue(cockpit.confirmed_filter(frame, SESSION))
                self.assertFalse(cockpit.confirmed_filter(frame, "bundle-pi"))
                self.assertFalse(cockpit.confirmed_filter(frame, SESSION + "-other"))
                self.assertFalse(cockpit.confirmed_filter(frame.replace(SESSION, SESSION + "▏"), SESSION))

    def test_filter_requires_one_selected_row_and_one_filtered_project_member(self):
        selected = "│›   Pi session                       Pi         Ready     │"
        for frame in (
            CONTOURED.replace(selected, "│    Pi session                       Pi         Ready     │"),
            CONTOURED.replace(selected, selected + "\n" + selected),
            CONTOURED.replace("No recorded project · 1", "No recorded project · 2"),
            CONTOURED.replace("No recorded project · 1", "No recorded project · 11"),
            CONTOURED.replace("No recorded project · 1", "No recorded project"),
        ):
            with self.subTest(frame=frame):
                self.assertFalse(cockpit.confirmed_filter(frame, SESSION))

    def test_wide_search_stops_at_the_task_contour_and_preserves_query_punctuation(self):
        wide = CONTOURED.replace(
            "╰ / bundle-pi-session ──────────────────────────────────────╯",
            "╰ / bundle-pi-session ──────────────────────────────────────╯ ╰ selected task ───╯",
        )
        self.assertEqual(cockpit.roster_search(wide), SESSION)
        self.assertTrue(cockpit.confirmed_filter(wide, SESSION))
        for query in ("fixture?[]:/query--", "fixture─"):
            with self.subTest(query=query):
                self.assertEqual(cockpit.roster_search(wide.replace(SESSION, query)), query)
        self.assertFalse(cockpit.confirmed_filter(wide.replace(SESSION, "bundle-pi-sess…"), SESSION))
        self.assertEqual(cockpit.roster_search("╰ / bundle-pi-session ─────"), "")

    def test_task_title_uses_header_boundaries_instead_of_a_fixed_character_count(self):
        for frame in (LEGACY, CONTOURED):
            with self.subTest(frame=frame):
                self.assertEqual(cockpit.selected_title(frame), "Pi session")
        no_provider = CONTOURED.replace(
            "│   Task                              Agent      Status    │",
            "│   Task                                         Status    │",
        ).replace(
            "│›   Pi session                       Pi         Ready     │",
            "│›   Pi session                                  Ready     │",
        )
        self.assertEqual(cockpit.selected_title(no_provider), "Pi session")

    def test_preview_content_is_not_a_second_selected_task_or_part_of_its_title(self):
        wide = CONTOURED.replace(
            "│›   Pi session                       Pi         Ready     │",
            "│›   Pi session                       Pi         Ready     │ │ › preview text │",
        )
        self.assertEqual(len(cockpit.selected_rows(wide)), 1)
        self.assertEqual(cockpit.selected_title(wide), "Pi session")
        self.assertEqual(cockpit.selected_task(wide), cockpit.selected_task(CONTOURED))

    def test_selection_proof_rejects_missing_or_ambiguous_selected_rows(self):
        for frame in ("no roster", CONTOURED + "\n│›   Another task │"):
            with self.subTest(frame=frame):
                with self.assertRaisesRegex(AssertionError, "exactly one selected task"):
                    cockpit.selected_task(frame)

    def test_project_counts_require_a_group_row_and_the_exact_count(self):
        self.assertTrue(cockpit.has_project_count(CONTOURED, 1))
        self.assertTrue(cockpit.has_project_count(LEGACY, 1))
        self.assertFalse(cockpit.has_project_count(CONTOURED, 2))
        self.assertFalse(cockpit.has_project_count("Details: No recorded project · 1", 1))
        self.assertEqual(cockpit.roster_search("Help mentions / bundle-pi-session"), "")


if __name__ == "__main__":
    unittest.main()
