"""Check ARM64 ELF LOAD alignment and uncompressed APK native-library offsets."""
import argparse
from pathlib import Path
import struct
import zipfile

PAGE = 16384


def check_elf(stream, name):
    header = stream.read(64)
    assert len(header) == 64 and header[:6] == b"\x7fELF\x02\x01", f"Not little-endian ELF64: {name}"
    assert struct.unpack_from("<H", header, 18)[0] == 183, f"Not ARM64: {name}"
    offset = struct.unpack_from("<Q", header, 32)[0]
    size, count = struct.unpack_from("<HH", header, 54)
    assert size >= 56 and 0 < count <= 128
    stream.seek(offset)
    table = stream.read(size * count)
    assert len(table) == size * count
    loads = 0
    for index in range(count):
        kind, _, file_offset, address, _, _, _, alignment = struct.unpack_from("<IIQQQQQQ", table, index * size)
        if kind == 1:
            loads += 1
            assert alignment >= PAGE and alignment & (alignment - 1) == 0, f"LOAD alignment {alignment:#x}: {name}"
            assert (address - file_offset) % PAGE == 0, f"LOAD offset/address mismatch: {name}"
    assert loads > 0
    print(f"PASS: {name}: {loads} ELF LOAD segments support 16 KiB pages")


def check_apk(path):
    with zipfile.ZipFile(path) as archive, path.open("rb") as raw:
        libraries = [entry for entry in archive.infolist() if entry.filename.endswith(".so")]
        assert libraries, "APK contains no native libraries"
        for entry in libraries:
            with archive.open(entry) as stream:
                check_elf(stream, entry.filename)
            if entry.compress_type == zipfile.ZIP_STORED:
                raw.seek(entry.header_offset)
                header = raw.read(30)
                assert header[:4] == b"PK\x03\x04"
                name_size, extra_size = struct.unpack_from("<HH", header, 26)
                offset = entry.header_offset + 30 + name_size + extra_size
                assert offset % PAGE == 0, f"APK library offset {offset}: {entry.filename}"
                print(f"PASS: {entry.filename}: APK data offset {offset} is 16 KiB aligned")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", type=Path, nargs="+")
    for path in parser.parse_args().files:
        if path.suffix == ".apk":
            check_apk(path)
        else:
            with path.open("rb") as stream:
                check_elf(stream, str(path))
