# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Binary layout shared by the TRAMA writer and reader.

Every struct here is normative in `docs/SPEC.md`. Keeping one definition stops the
compiler and the exporters from drifting apart field by field.
"""

from __future__ import annotations

import math
import struct

MAGIC = b"TRAMA\0\0\0"
FORMAT_VERSION = (0, 2, 0)
MINIMUM_READER_VERSION = (0, 1, 0)
HEADER = struct.Struct("<8s3H3HIQIIQ16s")
DIRECTORY = struct.Struct("<4sIIIIQQQIHBB12s")
COLUMN = struct.Struct("<IBBHIII")
ZSTD = 1

WORLD = 40075016.68557849
EXTENT = 65535
MAX_ZOOM = 14

F64, I64, STRING, BOOL, ENUM = 1, 2, 3, 4, 5
NODE_KIND, EDGE_KIND = 1, 2


def web_mercator(longitude: float, latitude: float) -> tuple[float, float]:
    latitude = max(min(float(latitude), 85.05112878), -85.05112878)
    y = math.log(math.tan((90 + latitude) * math.pi / 360)) / (math.pi / 180)
    return float(longitude) * WORLD / 360, y * WORLD / 360


def wgs84(x_m: float, y_m: float) -> tuple[float, float]:
    latitude = math.degrees(2 * math.atan(math.exp(y_m * 360 / WORLD * math.pi / 180)) - math.pi / 2)
    return x_m * 360 / WORLD, latitude


def tile_bounds(tile: tuple[int, int, int]) -> tuple[float, float, float]:
    z, x, y = tile
    width = WORLD / (1 << z)
    return -WORLD / 2 + x * width, WORLD / 2 - y * width, width


def tile_key(x_m: float, y_m: float, z: int) -> tuple[int, int, int]:
    tiles = 1 << z
    x = min(tiles - 1, max(0, int((x_m + WORLD / 2) / WORLD * tiles)))
    y = min(tiles - 1, max(0, int((WORLD / 2 - y_m) / WORLD * tiles)))
    return z, x, y


def quantize(point: tuple[float, float], tile: tuple[int, int, int]) -> tuple[int, int]:
    min_x, max_y, width = tile_bounds(tile)
    return (
        max(0, min(EXTENT, round((point[0] - min_x) / width * EXTENT))),
        max(0, min(EXTENT, round((max_y - point[1]) / width * EXTENT))),
    )


def unquantize(qx: int, qy: int, tile: tuple[int, int, int]) -> tuple[float, float]:
    min_x, max_y, width = tile_bounds(tile)
    return min_x + qx / EXTENT * width, max_y - qy / EXTENT * width


def pack_strings(values: list[str]) -> bytes:
    encoded = [value.encode() for value in values]
    return struct.pack("<I", len(encoded)) + b"".join(struct.pack("<I", len(item)) + item for item in encoded)


def unpack_strings(payload: bytes, offset: int) -> list[str]:
    count = struct.unpack_from("<I", payload, offset)[0]
    values = []
    cursor = offset + 4
    for _ in range(count):
        size = struct.unpack_from("<I", payload, cursor)[0]
        if cursor + 4 + size > len(payload):
            raise ValueError("string dictionary runs past the end of its section")
        values.append(payload[cursor + 4 : cursor + 4 + size].decode())
        cursor += 4 + size
    return values


def _crc32c_table() -> list[int]:
    table = []
    for byte in range(256):
        crc = byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82F63B78 if crc & 1 else 0)
        table.append(crc)
    return table


_CRC32C_TABLE = _crc32c_table()


def crc32c(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for byte in data:
        crc = _CRC32C_TABLE[(crc ^ byte) & 0xFF] ^ (crc >> 8)
    return (~crc) & 0xFFFFFFFF
