import tomllib
import unittest
from pathlib import Path


class ConsoleEntryPointTests(unittest.TestCase):
    def test_agenturm_is_the_only_installed_command(self):
        project = Path(__file__).resolve().parents[1] / "pyproject.toml"
        with project.open("rb") as source:
            scripts = tomllib.load(source)["project"]["scripts"]

        self.assertEqual(scripts, {"agenturm": "observatory.__main__:main"})


if __name__ == "__main__":
    unittest.main()
