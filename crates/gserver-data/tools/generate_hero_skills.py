#!/usr/bin/env python3
"""
Generate `crates/gserver-data/src/generated_hero_skills.rs` (and sync `com_atk` /
`angry_atk` onto `curated_templates.json` + `generated_consts.rs`) directly from
decrypted GServer (`vnsg2`) tables exported by `export_db_to_json.py`.

Zero third-party dependencies (Python 3 standard library only).
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Dict, List, Set

FALLBACK_NORMAL_SKILL_ID = 1001
FALLBACK_RAGE_SKILL_ID = 2001


def load_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def pos_u32(val: object) -> int:
    if isinstance(val, int) and not isinstance(val, bool) and val > 0:
        return val
    return 0


def collect_validated_skill_ids(export_dir: Path, tm_hero_rows: List[dict]) -> List[int]:
    valid: Set[int] = {FALLBACK_NORMAL_SKILL_ID, FALLBACK_RAGE_SKILL_ID}

    for r in tm_hero_rows:
        for col in ("com_atk", "angry_atk", "awake_com_atk", "awake_angry_atk"):
            v = pos_u32(r.get(col))
            if v > 0:
                valid.add(v)
        for col in ("com_atk_add", "trig_atk", "passive_ids"):
            for elem in r.get(col) or []:
                v = pos_u32(elem)
                if v > 0:
                    valid.add(v)

    disp_path = export_dir / "skill_display/skill_display.json"
    if disp_path.exists():
        for r in load_json(disp_path).get("rows", []):
            v = pos_u32(r.get("id"))
            if v > 0:
                valid.add(v)

    joint_path = export_dir / "jointSkill/jointSkill.json"
    if joint_path.exists():
        for r in load_json(joint_path).get("rows", []):
            for pair in r.get("skill_limit") or []:
                if isinstance(pair, list) and len(pair) >= 2:
                    v = pos_u32(pair[1])
                    if v > 0:
                        valid.add(v)

    pet_path = export_dir / "skill_pet/skill_pet.json"
    if pet_path.exists():
        for r in load_json(pet_path).get("rows", []):
            v = pos_u32(r.get("add_trigger_skill"))
            if v > 0:
                valid.add(v)

    soul_suit_path = export_dir / "soul_skill/soulSkill_suit.json"
    if soul_suit_path.exists():
        for r in load_json(soul_suit_path).get("rows", []):
            for pair in r.get("suit_skill") or []:
                if isinstance(pair, list) and pair:
                    v = pos_u32(pair[0])
                    if v > 0:
                        valid.add(v)
            for elem in r.get("soul_skill_id") or []:
                v = pos_u32(elem)
                if v > 0:
                    valid.add(v)

    skill_path = export_dir / "skill/skill.json"
    if skill_path.exists():
        for r in load_json(skill_path).get("rows", []):
            v = pos_u32(r.get("id"))
            if v > 0:
                valid.add(v)

    return sorted(valid)


def rust_escape_str(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def generate_rust_table(tm_hero_rows: List[dict], valid_skill_ids: List[int]) -> str:
    sorted_rows = sorted(tm_hero_rows, key=lambda row: int(row["id"]))

    lines: List[str] = [
        "// Auto-generated from authentic GServer (`vnsg2`) `timeMachine_hero.db`,",
        "// `skill_display.db`, `jointSkill.db`, `skill_pet.db`, and `soul_skill.db`.",
        "// Regenerate with `python3 crates/gserver-data/tools/generate_hero_skills.py`.",
        "",
        "use crate::hero_skills::HeroSkillEntry;",
        "",
        "#[allow(clippy::too_many_arguments)]",
        "const fn r(",
        "    htid: u32,",
        "    name_vi: &'static str,",
        "    proto: u32,",
        "    country: u8,",
        "    qual: u8,",
        "    prot: u8,",
        "    show: u8,",
        "    com: u32,",
        "    angry: u32,",
        "    aw_com: u32,",
        "    aw_angry: u32,",
        "    tray: u32,",
        "    gild: u32,",
        ") -> HeroSkillEntry {",
        "    HeroSkillEntry {",
        "        htid,",
        "        name_vi,",
        "        prototype_htid: proto,",
        "        country,",
        "        init_quality: qual,",
        "        is_protagonist: prot != 0,",
        "        is_show: show != 0,",
        "        com_atk: if com > 0 { Some(com) } else { None },",
        "        angry_atk: if angry > 0 { Some(angry) } else { None },",
        "        awake_com_atk: if aw_com > 0 { Some(aw_com) } else { None },",
        "        awake_angry_atk: if aw_angry > 0 { Some(aw_angry) } else { None },",
        "        tray_gold_htid: tray,",
        "        gild_gold_htid: gild,",
        "    }",
        "}",
        "",
        f"/// Complete authentic `timeMachine_hero` skill table ({len(sorted_rows)} rows, sorted by `htid`).",
        "#[rustfmt::skip]",
        "pub const HERO_SKILL_ENTRIES: &[HeroSkillEntry] = &[",
    ]

    for row in sorted_rows:
        hid = int(row["id"])
        name_vi = rust_escape_str(str(row.get("name") or "").strip())
        proto = pos_u32(row.get("prototype")) or hid
        country = pos_u32(row.get("country"))
        qual = pos_u32(row.get("init_quality"))
        prot = 1 if pos_u32(row.get("isProtagonist")) == 1 else 0
        show = 1 if pos_u32(row.get("is_show")) == 1 else 0
        com = pos_u32(row.get("com_atk"))
        angry = pos_u32(row.get("angry_atk"))
        aw_com = pos_u32(row.get("awake_com_atk"))
        aw_angry = pos_u32(row.get("awake_angry_atk"))
        tray = pos_u32(row.get("trayGold_id"))
        gild = pos_u32(row.get("gildGold_id"))
        lines.append(
            f'    r({hid}, "{name_vi}", {proto}, {country}, {qual}, {prot}, {show}, '
            f"{com}, {angry}, {aw_com}, {aw_angry}, {tray}, {gild}),"
        )

    lines.append("];")
    lines.append("")
    lines.append(
        f"/// Sorted deduplicated set of validated skill IDs ({len(valid_skill_ids)} entries)."
    )
    lines.append("#[rustfmt::skip]")
    lines.append("pub const VALIDATED_SKILL_IDS: &[u32] = &[")

    chunk_size = 10
    for i in range(0, len(valid_skill_ids), chunk_size):
        chunk = ", ".join(str(x) for x in valid_skill_ids[i : i + chunk_size])
        lines.append(f"    {chunk},")

    lines.append("];")
    lines.append("")
    return "\n".join(lines)


def sync_curated_files(crate_dir: Path, by_id: Dict[int, dict]) -> None:
    curated_path = crate_dir / "data/curated_templates.json"
    curated = load_json(curated_path)

    for p in curated.get("protagonists", []):
        sh = int(p["starter_htid"])
        row = by_id[sh]
        p["com_atk"] = int(row["com_atk"])
        p["angry_atk"] = int(row["angry_atk"])

    for h in curated.get("heroes", []):
        hid = int(h["htid"])
        row = by_id[hid]
        h["com_atk"] = int(row["com_atk"])
        h["angry_atk"] = int(row["angry_atk"])

    curated_path.write_text(
        json.dumps(curated, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )

    consts_path = crate_dir / "src/generated_consts.rs"
    consts_text = consts_path.read_text(encoding="utf-8")

    # Update ProtagonistTemplate blocks in generated_consts.rs
    def repl_prot(match: re.Match) -> str:
        body = match.group(1)
        sh_m = re.search(r"starter_htid:\s*(\d+),", body)
        if not sh_m:
            return match.group(0)
        sh = int(sh_m.group(1))
        row = by_id[sh]
        com = int(row["com_atk"])
        angry = int(row["angry_atk"])
        body = re.sub(r"\n\s*com_atk:\s*\d+,", "", body)
        body = re.sub(r"\n\s*angry_atk:\s*\d+,", "", body)
        body = body.rstrip() + f"\n        com_atk: {com},\n        angry_atk: {angry},\n    "
        return f"ProtagonistTemplate {{{body}}}"

    consts_text = re.sub(
        r"ProtagonistTemplate \{([^}]+)\}", repl_prot, consts_text
    )

    # Update HeroTemplate blocks in generated_consts.rs
    def repl_hero(match: re.Match) -> str:
        body = match.group(1)
        hid_m = re.search(r"htid:\s*(\d+),", body)
        if not hid_m:
            return match.group(0)
        hid = int(hid_m.group(1))
        row = by_id[hid]
        com = int(row["com_atk"])
        angry = int(row["angry_atk"])
        body = re.sub(r"\n\s*com_atk:\s*\d+,", "", body)
        body = re.sub(r"\n\s*angry_atk:\s*\d+,", "", body)
        body = body.rstrip() + f"\n        com_atk: {com},\n        angry_atk: {angry},\n    "
        return f"HeroTemplate {{{body}}}"

    consts_text = re.sub(r"HeroTemplate \{([^}]+)\}", repl_hero, consts_text)
    consts_path.write_text(consts_text, encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate embedded hero battle skill mappings for gserver-data."
    )
    parser.add_argument(
        "--export-dir",
        type=Path,
        required=True,
        help="Directory exported by tools/export_db_to_json.py",
    )
    parser.add_argument(
        "--crate-dir",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="Path to crates/gserver-data",
    )
    args = parser.parse_args()

    tm_hero_doc = load_json(args.export_dir / "timeMachine_hero/timeMachine_hero.json")
    tm_rows = tm_hero_doc["rows"]
    by_id = {int(r["id"]): r for r in tm_rows}

    valid_skill_ids = collect_validated_skill_ids(args.export_dir, tm_rows)
    out_rs = args.crate_dir / "src/generated_hero_skills.rs"
    out_rs.write_text(generate_rust_table(tm_rows, valid_skill_ids), encoding="utf-8")

    sync_curated_files(args.crate_dir, by_id)
    print(
        f"Generated {out_rs} ({len(tm_rows)} hero rows, {len(valid_skill_ids)} validated skill IDs)"
    )


if __name__ == "__main__":
    main()
