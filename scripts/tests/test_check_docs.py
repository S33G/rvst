"""Exercise the offline documentation checker through its public CLI."""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "check-docs.py"


class CheckDocsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def write(self, name, text=""):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def check(self, *paths):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(self.root), *paths],
            capture_output=True, text=True, cwd=self.root,
        )

    def test_nested_links_images_and_encoded_paths(self):
        self.write("README.md", "[![Preview](assets/preview.png)](docs/deep/start.md)\n"
                   "[API](docs/API_(draft).md)\n"
                   "[Spaced](<docs/With spaces.md#caf%C3%A9--rust>)\n"
                   "[Encoded](docs/With%20spaces.md#caf%C3%A9--rust)\n")
        self.write("assets/preview.png")
        self.write("docs/deep/start.md", "[Home](../../README.md)\n")
        self.write("docs/API_(draft).md")
        self.write("docs/With spaces.md", "# Café & `Rust`\n")
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("4 Markdown files", result.stdout)
        self.assertIn("6 local links", result.stdout)

    def test_missing_file_and_bad_fragment_report_source_line(self):
        self.write("README.md", "# Start\n[Missing](docs/gone.md)\n[Bad](docs/guide.md#gone)\n")
        self.write("docs/guide.md", "# Here\n")
        result = self.check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("README.md:2: missing local target", result.stdout)
        self.assertIn("README.md:3: missing heading #gone", result.stdout)

    def test_multiline_destinations_report_the_link_line(self):
        self.write("README.md", "# Start\n\n[Broken](\nmissing.md)\n"
                   "[Bad anchor](\ndocs/guide.md#gone)\n"
                   "![Missing image](\nassets/missing.png)\n")
        self.write("docs/guide.md", "# Here\n")
        result = self.check()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("README.md:3: missing local target: missing.md", result.stdout)
        self.assertIn("README.md:5: missing heading #gone", result.stdout)
        self.assertIn("README.md:7: missing local target: assets/missing.png", result.stdout)

    def test_multiline_links_preserve_code_and_external_exclusions(self):
        self.write("README.md", "[Valid](\n<docs/With spaces.md#intro>)\n"
                   "[Remote](\nhttps://example.com/guide)\n"
                   "`[inline](\nmissing.md)`\n"
                   "~~~md\n[code](\nmissing.md)\n~~~\n")
        self.write("docs/With spaces.md", "# Intro\n")
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("1 local links", result.stdout)

    def test_ignores_code_and_all_external_schemes(self):
        self.write("README.md", "`[inline](missing.md)`\n``a ` [nested](missing.md)``\n"
                   "```md\n# Hidden\n[code](missing.md)\n```\n"
                   "    [indented](missing.md)\n"
                   "[web](https://example.com/a) [mail](mailto:x@y.test)\n"
                   "[custom](custom+docs:thing) [remote](//example.com/a)\n")
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0 local links", result.stdout)

    def test_duplicate_heading_slugs_and_setext_headings(self):
        self.write("README.md", "# Repeat\n# Repeat\n# Repeat-1\n"
                   "Setext heading\n--------------\n"
                   "[A](#repeat) [B](#repeat-1) [C](#repeat-1-1) [D](#setext-heading)\n")
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_fenced_headings_do_not_create_anchors(self):
        self.write("README.md", "```md\n# Hidden\n```\n[Bad](#hidden)\n")
        result = self.check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("README.md:4: missing heading #hidden", result.stdout)

    def test_shorter_or_other_fences_do_not_close_block(self):
        self.write("README.md", "````md\n```\n[hidden](gone.md)\n~~~~\n`````\n"
                   "~~~md\n```\n[hidden](gone.md)\n~~~~\n")
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0 local links", result.stdout)

    def test_unclosed_fence_reports_opening_line(self):
        self.write("README.md", "# Start\n\n~~~~md\n~~~\n")
        result = self.check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("README.md:3: unclosed ~~~~ fence", result.stdout)

    def test_reference_definitions_check_targets_and_line_numbers(self):
        self.write("README.md", "[Guide][guide]\n![Logo][logo]\n\n"
                   "[guide]: <docs/With spaces.md#intro> 'Guide title'\n"
                   "[logo]: assets/logo.png\n[broken]: docs/missing.md\n")
        self.write("docs/With spaces.md", "# Intro\n")
        self.write("assets/logo.png")
        result = self.check()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("README.md:6: missing local target", result.stdout)
        self.assertNotIn("README.md:4:", result.stdout)
        self.assertNotIn("README.md:5:", result.stdout)

    def test_rejects_absolute_and_repo_escaping_local_links(self):
        self.write("README.md", "[absolute](/etc/passwd)\n"
                   "[escape](../outside.md)\n[encoded](%2e%2e/outside.md)\n")
        result = self.check()
        self.assertEqual(result.returncode, 1, result.stderr)
        for line in [1, 2, 3]:
            self.assertIn(f"README.md:{line}: local target leaves repository", result.stdout)

    def test_rejects_windows_absolute_paths_on_every_platform(self):
        self.write("README.md", "[absolute](C:/Windows/win.ini)\n")
        result = self.check()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("README.md:1: local target leaves repository", result.stdout)

    def test_selected_paths_do_not_scan_other_documents(self):
        self.write("README.md", "[broken](gone.md)\n")
        self.write("docs/guide.md", "# Intro\n[Self](#intro)\n")
        result = self.check("docs/guide.md")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("1 Markdown files", result.stdout)
        self.assertIn("1 local links", result.stdout)


if __name__ == "__main__":
    unittest.main()
