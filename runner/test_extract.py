"""PDF pages without text must be named, never skipped in silence."""
import importlib
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))


def text_page(text):
    """One page whose content stream draws this text with a standard font."""
    from pypdf.generic import (ArrayObject, DecodedStreamObject, DictionaryObject, FloatObject,
                               NameObject)
    stream = DecodedStreamObject()
    stream.set_data(f"BT /F1 12 Tf 10 100 Td ({text}) Tj ET".encode())
    font = DictionaryObject({NameObject("/Type"): NameObject("/Font"),
                             NameObject("/Subtype"): NameObject("/Type1"),
                             NameObject("/BaseFont"): NameObject("/Helvetica")})
    return stream, font


class ExtractPages(unittest.TestCase):
    def setUp(self):
        try:
            import pypdf  # noqa: F401
        except ImportError:
            self.skipTest("pypdf is installed only in the sandbox image")
        sys.modules.pop("extract", None)
        arguments, sys.argv = sys.argv, ["extract.py"]
        self.addCleanup(setattr, sys, "argv", arguments)
        self.extract = importlib.import_module("extract")

    def pdf(self, pages):
        from pypdf import PdfWriter
        from pypdf.generic import DictionaryObject, NameObject
        writer = PdfWriter()
        for text in pages:
            page = writer.add_blank_page(width=200, height=200)
            if text:
                stream, font = text_page(text)
                page[NameObject("/Contents")] = writer._add_object(stream)
                page[NameObject("/Resources")] = DictionaryObject({
                    NameObject("/Font"): DictionaryObject({NameObject("/F1"): font})})
        handle = tempfile.NamedTemporaryFile(suffix=".pdf", delete=False)
        writer.write(handle)
        handle.close()
        self.addCleanup(Path(handle.name).unlink)
        return Path(handle.name)

    def test_a_page_without_text_is_marked_and_listed(self):
        body = "This page carries a readable text layer."
        state = self.extract.extract(self.pdf([body, None, body, "12"]))
        text = "".join(self.extract.output)
        self.assertEqual(state, "ready")
        self.assertEqual(self.extract.page_count, 4)
        self.assertEqual(self.extract.pages_without_text, [2, 4])
        self.assertIn("[Page 1]", text)
        self.assertIn("[Page 2: no text]", text)
        self.assertIn("[Page 4: no text]", text)

    def test_a_scan_is_not_extractable_but_every_page_is_marked(self):
        state = self.extract.extract(self.pdf([None, None]))
        text = "".join(self.extract.output)
        self.assertEqual(state, "non_extractable")
        self.assertEqual(self.extract.pages_without_text, [1, 2])
        self.assertIn("[Page 2: no text]", text)

    def test_plain_text_never_reaches_the_parser(self):
        with tempfile.NamedTemporaryFile(suffix=".md", delete=False) as handle:
            handle.write(b"# plain\n")
        self.addCleanup(Path(handle.name).unlink)
        self.assertEqual(self.extract.extract(Path(handle.name)), "unsupported")


if __name__ == "__main__":
    unittest.main()
