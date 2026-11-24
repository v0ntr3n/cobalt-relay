#!/usr/bin/env python3
"""
GServer (vnsg2 / Babeltime) SQLite `.db` Decryption & JSON Exporter.

Decrypts both:
  1. Whole-file `0xFEFEFEFE` encrypted `.db` containers (via `bt_decrypt` / `decrypt_basic`).
  2. Row-level `babeltime` columns inside SQLite tables (hex-encoded `FEFEFEFE...` or raw
     `0xFEFEFEFE` blobs, matching `CConfigManager.cpp` -> `CTable::getRowById`).

Parses Babeltime's `id = "0"` schema metadata row (`col1,col2,...\\~type1,type2,...`) and
exports clean, structured UTF-8 JSON tables + `manifest.json` consumable by `gserver-data`.

Zero third-party dependencies (uses Python 3 standard library only).
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import struct
import sys
import tempfile
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

BT_DECRYPT_MAGIC = 0xFEFEFEFE
BT_DECRYPT_HEADER_SIZE = 32

# Exact 16-slot key table from `libcocos2dlua.so` (`07_bt_decrypt_keys.inc` @ `off_CC01A0[0..15]`)
BT_KEY_TABLES: Tuple[bytes, ...] = (
    # [ 0] encrypt_hash_header (len=32)
    bytes([
        0x70, 0x96, 0xB2, 0x9D, 0xD0, 0x52, 0x19, 0x9A, 0x79, 0xC6, 0xD9, 0x6C, 0x6B, 0xBD, 0xDC, 0x4C,
        0x51, 0x52, 0xAC, 0xFA, 0x99, 0x5C, 0x65, 0x71, 0x9B, 0x06, 0x80, 0x4A, 0x02, 0x6E, 0x20, 0x02,
    ]),
    # [ 1] encrypt_hash_tail (len=32)
    bytes([
        0x57, 0xAE, 0x6D, 0x16, 0x45, 0x2E, 0x43, 0x74, 0x61, 0x76, 0xF0, 0xA4, 0x7F, 0x8C, 0x91, 0x79,
        0xBF, 0x6E, 0x3D, 0x76, 0xA6, 0x88, 0x8E, 0x07, 0x2C, 0x12, 0xED, 0x22, 0x86, 0xCE, 0x98, 0xF7,
    ]),
    # [ 2] encrypt_hash_odd (len=32)
    bytes([
        0x96, 0x9D, 0x52, 0x9A, 0xC6, 0x6C, 0xBD, 0x4C, 0x52, 0xFA, 0x5C, 0x71, 0x06, 0x4A, 0x6E, 0x02,
        0x10, 0x80, 0x08, 0x40, 0x72, 0x4C, 0x56, 0x29, 0x97, 0x4A, 0x5B, 0xE0, 0x58, 0x53, 0x4D, 0x38,
    ]),
    # [ 3] encrypt_hash_even (len=32)
    bytes([
        0x70, 0xB2, 0xD0, 0x19, 0x79, 0xD9, 0x6B, 0xDC, 0x51, 0xAC, 0x99, 0x65, 0x9B, 0x80, 0x02, 0x20,
        0x40, 0x04, 0x04, 0x01, 0x99, 0x05, 0x10, 0x9E, 0xF0, 0x70, 0x04, 0x87, 0x27, 0xB2, 0x53, 0xAF,
    ]),
    # [ 4] encrypt_hash_fibonacci (len=10)
    bytes([
        0x96, 0xB2, 0x9D, 0x52, 0x79, 0xBD, 0x5C, 0x04, 0xE0, 0xCA,
    ]),
    # [ 5] encrypt_hash_prime (len=32)
    bytes([
        0xB2, 0x9D, 0x52, 0x9A, 0x6C, 0xBD, 0x52, 0xFA, 0x71, 0x6E, 0x02, 0x08, 0x72, 0x4C, 0x29, 0x5B,
        0x53, 0x4D, 0x3F, 0x2F, 0x33, 0xF9, 0xB6, 0xCA, 0x63, 0x6D, 0x43, 0x61, 0x7F, 0x91, 0x3D, 0x2C,
    ]),
    # [ 6] encrypt_hash_odds (len=32)
    bytes([
        0x9D, 0x19, 0x79, 0xD9, 0x6B, 0xBD, 0x51, 0x99, 0x65, 0x71, 0x9B, 0x01, 0x99, 0x05, 0x29, 0x97,
        0x5B, 0x87, 0x27, 0x53, 0x53, 0x4D, 0xAF, 0xCB, 0x3F, 0xAD, 0x2F, 0xB3, 0x33, 0x43, 0x89, 0x1B,
    ]),
    # [ 7] encrypt_hash_evens (len=32)
    bytes([
        0x70, 0x96, 0xB2, 0xD0, 0x52, 0x9A, 0xC6, 0x6C, 0xDC, 0x4C, 0x52, 0xAC, 0xFA, 0x5C, 0x06, 0x80,
        0x4A, 0x02, 0x6E, 0x20, 0x02, 0x40, 0x10, 0x04, 0x80, 0x04, 0x08, 0x40, 0x72, 0x4C, 0x10, 0x56,
    ]),
    # [ 8] encrypt_hash_tail_odd (len=32)
    bytes([
        0x74, 0x98, 0x86, 0xED, 0x2C, 0x8E, 0xA6, 0x3D, 0xBF, 0x91, 0x7F, 0xF0, 0x61, 0x43, 0x45, 0x6D,
        0x57, 0x63, 0x76, 0xCA, 0x5F, 0x4B, 0xB6, 0x5D, 0xF9, 0x1B, 0x43, 0x33, 0x2F, 0x40, 0x3F, 0xCB,
    ]),
    # [ 9] encrypt_hash_tail_even (len=32)
    bytes([
        0xF7, 0xCE, 0x22, 0x12, 0x07, 0x88, 0x76, 0x6E, 0x79, 0x8C, 0xA4, 0x76, 0x74, 0x2E, 0x16, 0xAE,
        0xAF, 0x18, 0xB3, 0xA3, 0x98, 0x38, 0x44, 0x33, 0xEC, 0x89, 0x7C, 0xB3, 0xAD, 0x16, 0x3E, 0xBE,
    ]),
    # [10] encrypt_hash_tail_odds (len=32)
    bytes([
        0xF7, 0xED, 0x07, 0x3D, 0xBF, 0x79, 0x91, 0x7F, 0x61, 0x43, 0x45, 0x6D, 0x57, 0xAF, 0x63, 0xB3,
        0xA3, 0x5F, 0x4B, 0x5D, 0x33, 0xF9, 0x1B, 0x89, 0x43, 0x33, 0xB3, 0x2F, 0xAD, 0x3F, 0xCB, 0xAF,
    ]),
    # [11] encrypt_hash_tail_evens (len=32)
    bytes([
        0x74, 0x98, 0xCE, 0x86, 0x22, 0x12, 0x2C, 0x8E, 0x88, 0xA6, 0x76, 0x6E, 0x8C, 0xA4, 0xF0, 0x76,
        0x74, 0x2E, 0x16, 0xAE, 0x18, 0x76, 0xCA, 0x98, 0x38, 0xB6, 0x44, 0xEC, 0x7C, 0x40, 0x16, 0x3E,
    ]),
    # [12] encrypt_hash_odd_sub (len=32)
    bytes([
        0xDA, 0x15, 0x7E, 0x7F, 0xB3, 0x6D, 0xAE, 0x90, 0xFF, 0xB2, 0x3D, 0xF4, 0x95, 0x36, 0x94, 0x1E,
        0x30, 0x84, 0xFC, 0xC1, 0x27, 0xB9, 0xBA, 0x75, 0x59, 0x26, 0xA9, 0xA7, 0xCF, 0x5F, 0x06, 0x77,
    ]),
    # [13] encrypt_hash_even_sub (len=32)
    bytes([
        0xE4, 0xCD, 0x39, 0x21, 0xED, 0x01, 0xE1, 0xFB, 0xA6, 0x61, 0xF7, 0xD6, 0x86, 0x48, 0x4E, 0xC2,
        0x0C, 0x7C, 0x07, 0xA7, 0x6D, 0x3C, 0xB8, 0x39, 0x27, 0x46, 0xD4, 0xB9, 0xA6, 0x00, 0x9E, 0x7A,
    ]),
    # [14] encrypt_hash_odd_add (len=32)
    bytes([
        0x06, 0x4F, 0x22, 0xB3, 0x3F, 0x45, 0x28, 0x28, 0xA3, 0xA6, 0xF5, 0xD6, 0xA1, 0xCA, 0x70, 0x22,
        0x50, 0x84, 0x0C, 0x41, 0x0B, 0x51, 0x66, 0xC7, 0x87, 0xBA, 0x5F, 0x67, 0x7F, 0x05, 0xA0, 0xE7,
    ]),
    # [15] encrypt_hash_even_add (len=32)
    bytes([
        0x48, 0x6D, 0x6B, 0x13, 0x9F, 0xD7, 0x99, 0x9D, 0xFE, 0x93, 0xC1, 0x0C, 0x86, 0x4C, 0x8E, 0x42,
        0x14, 0x84, 0x09, 0xD9, 0x77, 0x5C, 0xF4, 0x19, 0x07, 0x4E, 0xE2, 0x07, 0x0A, 0xA6, 0xFC, 0xF6,
    ]),
)


def is_bt_encrypted(data: bytes) -> bool:
    if len(data) < BT_DECRYPT_HEADER_SIZE:
        return False
    magic = struct.unpack_from("<I", data, 0)[0]
    return magic == BT_DECRYPT_MAGIC


def bt_decrypt(data: bytes) -> bytes:
    """Decrypts an GServer 0xFEFEFEFE container buffer, or returns plaintext unchanged."""
    if not is_bt_encrypted(data):
        return data

    magic, header_payload_len = struct.unpack_from("<II", data, 0)
    algo_index = data[8]
    key_index = data[9]
    actual_payload_len = len(data) - BT_DECRYPT_HEADER_SIZE

    if actual_payload_len != header_payload_len:
        raise ValueError(
            f"bt_decrypt payload length mismatch: header={header_payload_len}, actual={actual_payload_len}"
        )
    if algo_index not in (0, 1):
        raise ValueError(f"bt_decrypt unsupported algo_index={algo_index}")
    if key_index >= len(BT_KEY_TABLES):
        raise ValueError(f"bt_decrypt invalid key_index={key_index}")

    key = BT_KEY_TABLES[key_index]
    key_len = len(key)
    out = bytearray(data[BT_DECRYPT_HEADER_SIZE:])
    if actual_payload_len > 0:
        offset = actual_payload_len % key_len
        for i in range(0, actual_payload_len, 2):
            out[i] ^= key[(i + offset) % key_len]
    return bytes(out)


def is_even_hex(s: str) -> bool:
    if len(s) < 64 or (len(s) % 2) != 0:
        return False
    try:
        bytes.fromhex(s[:16])
    except ValueError:
        return False
    return True


def decrypt_cell_value(raw_val: Any, is_last_col: bool) -> str:
    """Matches CTable::getRowById in CConfigManager.cpp."""
    if raw_val is None:
        return ""
    if isinstance(raw_val, bytes):
        if is_last_col and is_bt_encrypted(raw_val):
            return bt_decrypt(raw_val).decode("utf-8", errors="replace")
        return raw_val.decode("utf-8", errors="replace")

    text = str(raw_val)
    if not is_last_col or not text:
        return text

    trimmed = text.rstrip(" \t\r\n")
    if len(trimmed) >= 64 and trimmed[:8].upper() == "FEFEFEFE" and (len(trimmed) % 2) == 0:
        try:
            decoded = bytes.fromhex(trimmed)
            if is_bt_encrypted(decoded):
                return bt_decrypt(decoded).decode("utf-8", errors="replace")
        except ValueError:
            pass
    return text


def split_unescaped_tilde(value: str) -> List[str]:
    """
    Splits Babeltime `\\~` array strings in data cells:
    In GServer `babeltime` rows, `\\~` separates array elements when column type is `array`,
    while in text descriptions `\\~` represents an escaped comma/pause.
    """
    if not value:
        return []
    return [part for part in value.split("\\~") if part != ""]


def coerce_scalar(val: str) -> Any:
    if val == "":
        return None
    if val == "0" or (val.lstrip("-").isdigit() and not (val.lstrip("-").startswith("0") and len(val.lstrip("-")) > 1)):
        try:
            return int(val)
        except ValueError:
            pass
    return val


def parse_array_value(val: str) -> List[Any]:
    items = split_unescaped_tilde(val)
    out: List[Any] = []
    for item in items:
        if "|" in item:
            out.append([coerce_scalar(p) for p in item.split("|")])
        else:
            out.append(coerce_scalar(item))
    return out


def export_sqlite_db(db_path: Path, out_dir: Path, pretty: bool = False) -> Dict[str, Any]:
    raw_bytes = db_path.read_bytes()
    was_file_encrypted = is_bt_encrypted(raw_bytes)
    sqlite_bytes = bt_decrypt(raw_bytes) if was_file_encrypted else raw_bytes

    if not sqlite_bytes.startswith(b"SQLite format 3\x00"):
        raise ValueError(f"{db_path} is not a valid SQLite3 database after bt_decrypt check")

    tmp_file: Optional[Path] = None
    target_db_path = db_path
    if was_file_encrypted:
        fd, tmp_name = tempfile.mkstemp(prefix="gserver_dec_", suffix=".db")
        os.close(fd)
        tmp_file = Path(tmp_name)
        tmp_file.write_bytes(sqlite_bytes)
        target_db_path = tmp_file

    db_stem = db_path.stem
    db_out_dir = out_dir / db_stem
    db_out_dir.mkdir(parents=True, exist_ok=True)

    tables_summary: List[Dict[str, Any]] = []
    try:
        conn = sqlite3.connect(f"file:{target_db_path}?mode=ro", uri=True)
        cur = conn.cursor()
        table_rows = cur.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        ).fetchall()

        for (table_name,) in table_rows:
            cur.execute(f'SELECT * FROM "{table_name}"')
            sql_cols = [desc[0] for desc in (cur.description or [])]
            all_rows = cur.fetchall()
            if not sql_cols:
                continue

            has_babeltime_col = sql_cols[-1] == "babeltime"
            schema_cols: List[str] = []
            schema_types: List[str] = []
            records: List[Dict[str, Any]] = []

            for r in all_rows:
                row_id_str = str(r[0]) if r[0] is not None else ""
                last_decrypted = decrypt_cell_value(r[-1], is_last_col=True) if has_babeltime_col else ""

                if has_babeltime_col and row_id_str == "0":
                    parts = last_decrypted.split("\\~")
                    schema_cols = parts[0].split(",") if parts and parts[0] else []
                    schema_types = parts[1].split(",") if len(parts) > 1 else [""] * len(schema_cols)
                    if len(schema_types) < len(schema_cols):
                        schema_types.extend([""] * (len(schema_cols) - len(schema_types)))
                    continue

                rec: Dict[str, Any] = {"id": coerce_scalar(row_id_str)}
                if has_babeltime_col:
                    for idx, scol in enumerate(sql_cols[1:-1], start=1):
                        rec[scol] = coerce_scalar(decrypt_cell_value(r[idx], is_last_col=False))
                    vals = last_decrypted.split(",")
                    for idx, cname in enumerate(schema_cols):
                        if not cname:
                            continue
                        raw_field = vals[idx] if idx < len(vals) else ""
                        ctype = schema_types[idx] if idx < len(schema_types) else ""
                        if ctype == "array":
                            rec[cname] = parse_array_value(raw_field)
                        else:
                            rec[cname] = coerce_scalar(raw_field.replace("\\~", ", "))
                else:
                    for idx, scol in enumerate(sql_cols[1:], start=1):
                        rec[scol] = coerce_scalar(decrypt_cell_value(r[idx], is_last_col=(idx == len(sql_cols) - 1)))

                records.append(rec)

            table_payload = {
                "database": db_stem,
                "table": table_name,
                "file_encrypted": was_file_encrypted,
                "babeltime_packed": has_babeltime_col,
                "columns": [
                    {"name": cname, "kind": schema_types[i] if i < len(schema_types) and schema_types[i] else "scalar"}
                    for i, cname in enumerate(schema_cols)
                    if cname
                ] if has_babeltime_col else [{"name": c, "kind": "scalar"} for c in sql_cols],
                "row_count": len(records),
                "rows": records,
            }

            table_json_path = db_out_dir / f"{table_name}.json"
            with table_json_path.open("w", encoding="utf-8") as f:
                if pretty:
                    json.dump(table_payload, f, ensure_ascii=False, indent=2)
                else:
                    json.dump(table_payload, f, ensure_ascii=False, separators=(",", ":"))

            tables_summary.append({
                "database": db_stem,
                "table": table_name,
                "relative_path": f"{db_stem}/{table_name}.json",
                "row_count": len(records),
                "column_count": len(table_payload["columns"]),
            })

        conn.close()
    finally:
        if tmp_file is not None and tmp_file.exists():
            tmp_file.unlink()

    return {
        "database": db_stem,
        "source_file": db_path.name,
        "file_encrypted": was_file_encrypted,
        "tables": tables_summary,
    }


def export_directory(db_dir: Path, out_dir: Path, pretty: bool = False) -> Dict[str, Any]:
    out_dir.mkdir(parents=True, exist_ok=True)
    db_files = sorted(p for p in db_dir.glob("*.db") if p.is_file())
    databases: List[Dict[str, Any]] = []
    all_tables: List[Dict[str, Any]] = []

    for db_path in db_files:
        info = export_sqlite_db(db_path, out_dir, pretty=pretty)
        databases.append({
            "database": info["database"],
            "source_file": info["source_file"],
            "file_encrypted": info["file_encrypted"],
            "table_count": len(info["tables"]),
        })
        all_tables.extend(info["tables"])

    manifest = {
        "format_version": 1,
        "generator": "gserver-data/export_db_to_json.py",
        "database_count": len(databases),
        "table_count": len(all_tables),
        "databases": databases,
        "tables": all_tables,
    }
    manifest_path = out_dir / "manifest.json"
    with manifest_path.open("w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)
    return manifest


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="Decrypt and export GServer (vnsg2) SQLite .db tables to JSON for gserver-data."
    )
    parser.add_argument(
        "--db-dir",
        type=Path,
        required=True,
        help="Input directory containing GServer client .db files",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        required=True,
        help="Output directory for exported JSON tables and manifest.json",
    )
    parser.add_argument(
        "--pretty",
        action="store_true",
        help="Pretty-print exported JSON files with 2-space indentation",
    )
    args = parser.parse_args(argv)

    if not args.db_dir.is_dir():
        print(f"Error: --db-dir '{args.db_dir}' is not a directory", file=sys.stderr)
        return 1

    manifest = export_directory(args.db_dir, args.out_dir, pretty=args.pretty)
    print(
        f"Exported {manifest['database_count']} .db files ({manifest['table_count']} tables) "
        f"to {args.out_dir}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
