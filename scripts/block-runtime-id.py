#!/usr/bin/env python3
"""Re-derives a block's runtime id for the target protocol.

A runtime id is a block state's index in the version's canonical, ordered list — not a
stable number and not one that can be guessed. `minecraft:air` is 13094, not 0.

    scripts/block-runtime-id.py minecraft:stone minecraft:air

Reads the canonical list from pmmp/BedrockData, which tracks the same protocol this
server targets. Nothing is cached: the whole point is that the answer is a function of
the version, and a stale copy would silently answer for the wrong one.
"""

import sys
import urllib.request

SOURCE = "https://raw.githubusercontent.com/pmmp/BedrockData/master/canonical_block_states.nbt"


class Reader:
    """Network NBT: varint lengths, zigzag integers, little-endian floats."""

    def __init__(self, data: bytes) -> None:
        self.data = data
        self.pos = 0

    def byte(self) -> int:
        value = self.data[self.pos]
        self.pos += 1
        return value

    def varint(self) -> int:
        value = shift = 0
        while True:
            byte = self.byte()
            value |= (byte & 0x7F) << shift
            if not byte & 0x80:
                return value
            shift += 7

    def zigzag(self) -> int:
        value = self.varint()
        return (value >> 1) ^ -(value & 1)

    def string(self) -> str:
        length = self.varint()
        text = self.data[self.pos : self.pos + length].decode("utf8", "replace")
        self.pos += length
        return text

    def skip(self, tag: int) -> None:
        if tag == 1:
            self.pos += 1
        elif tag == 2:
            self.pos += 2
        elif tag in (3, 4):
            self.zigzag()
        elif tag == 5:
            self.pos += 4
        elif tag == 6:
            self.pos += 8
        elif tag == 7:
            self.pos += self.varint()
        elif tag == 8:
            self.string()
        elif tag == 9:
            item = self.byte()
            for _ in range(self.varint()):
                self.skip(item)
        elif tag == 10:
            while True:
                inner = self.byte()
                if inner == 0:
                    return
                self.string()
                self.skip(inner)
        elif tag == 11:
            for _ in range(self.varint()):
                self.zigzag()
        else:
            raise ValueError(f"unknown NBT tag {tag} at {self.pos}")


def runtime_ids(data: bytes, wanted: set[str]) -> dict[str, int]:
    """Walks the concatenated root compounds, noting where each name first appears."""
    reader = Reader(data)
    found: dict[str, int] = {}
    index = 0

    while reader.pos < len(data):
        assert reader.byte() == 10, "every entry is a compound"
        reader.string()

        name = None
        while True:
            tag = reader.byte()
            if tag == 0:
                break
            key = reader.string()
            if key == "name" and tag == 8:
                name = reader.string()
            else:
                reader.skip(tag)

        # The first state of a block is its default, which is what a flat world wants.
        if name in wanted and name not in found:
            found[name] = index
        index += 1

    print(f"{index} block states in the canonical list", file=sys.stderr)
    return found


def main() -> int:
    wanted = set(sys.argv[1:]) or {"minecraft:air", "minecraft:stone"}
    with urllib.request.urlopen(SOURCE, timeout=120) as response:
        data = response.read()

    found = runtime_ids(data, wanted)
    for name in sorted(wanted):
        if name in found:
            print(f"{name} {found[name]}")
        else:
            print(f"{name} NOT FOUND", file=sys.stderr)
    return 0 if len(found) == len(wanted) else 1


if __name__ == "__main__":
    raise SystemExit(main())
