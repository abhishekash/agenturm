"""Python framing matches the Rust crate's fixed wire vectors and errors."""
import io
import unittest

from observatory.plugin_frames import FrameError, MAX_FRAME_BYTES, read_frame, write_frame


class SlowReader(io.BytesIO):
    def read(self, size=-1):
        return super().read(min(size, 1) if size >= 0 else 1)


class SlowWriter(io.BytesIO):
    def write(self, raw):
        return super().write(raw[:1])


class FrameTests(unittest.TestCase):
    def test_exact_wire_vector_partial_reads_and_clean_eof(self):
        stream = SlowWriter()
        write_frame(stream, '{"id":"α"}')
        write_frame(stream, "{}")
        self.assertEqual(stream.getvalue(),
                         b'\x00\x00\x00\x0b{"id":"\xce\xb1"}\x00\x00\x00\x02{}')
        reader = SlowReader(stream.getvalue())
        self.assertEqual(read_frame(reader), '{"id":"α"}')
        self.assertEqual(read_frame(reader), "{}")
        self.assertIsNone(read_frame(reader))

    def test_rejects_truncation_oversize_invalid_utf8_and_empty(self):
        for raw in (b"\x00\x00", b"\x00\x00\x00\x03x"):
            with self.subTest(raw=raw), self.assertRaisesRegex(FrameError, "ended before"):
                read_frame(io.BytesIO(raw))
        oversized = io.BytesIO(b"\x00\x00\x01\x00secret")
        with self.assertRaisesRegex(FrameError, "exceeds"):
            read_frame(oversized, max_bytes=8)
        self.assertEqual(oversized.tell(), 4)
        with self.assertRaisesRegex(FrameError, "UTF-8"):
            read_frame(io.BytesIO(b"\x00\x00\x00\x01\xff"))
        with self.assertRaisesRegex(FrameError, "empty"):
            read_frame(io.BytesIO(b"\x00\x00\x00\x00"))
        with self.assertRaisesRegex(FrameError, "empty"):
            write_frame(io.BytesIO(), "")
        with self.assertRaisesRegex(FrameError, "positive"):
            read_frame(io.BytesIO(), max_bytes=0)
        with self.assertRaisesRegex(FrameError, "UTF-8"):
            write_frame(io.BytesIO(), "\ud800")

    def test_global_limit_cannot_be_increased_by_peer(self):
        stream = io.BytesIO((MAX_FRAME_BYTES + 1).to_bytes(4, "big"))
        with self.assertRaisesRegex(FrameError, "exceeds"):
            read_frame(stream, max_bytes=MAX_FRAME_BYTES * 2)
        self.assertEqual(stream.tell(), 4)


if __name__ == "__main__":
    unittest.main()
