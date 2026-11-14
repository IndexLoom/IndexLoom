#!/usr/bin/env python3
"""Build real canonical and ALL variant YAML using a separate official Indexers checkout.

Run with that checkout's uv environment. Nothing is downloaded or imported during unit tests.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys


def revision(root):
    return subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("catalog", "jackett", "prowlarr", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    spec = importlib.util.spec_from_file_location("official_indexers", args.catalog / "tools/import_upstreams.py")
    importer = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = importer
    spec.loader.exec_module(importer)
    jackett_revision, prowlarr_revision = revision(args.jackett), revision(args.prowlarr)
    jackett = importer.discover(project=importer.JACKETT_PROJECT, repository=importer.JACKETT_REPOSITORY,
        revision=jackett_revision, repository_root=args.jackett,
        definition_root=args.jackett / "src/Jackett.Common/Definitions", recursive=False)
    prowlarr = importer.discover(project=importer.PROWLARR_PROJECT, repository=importer.PROWLARR_REPOSITORY,
        revision=prowlarr_revision, repository_root=args.prowlarr,
        definition_root=args.prowlarr / "definitions", recursive=True)
    if len(jackett) <= 500 or len(prowlarr) <= 500:
        raise ValueError(f"incomplete source catalogs: Jackett={len(jackett)}, Prowlarr={len(prowlarr)}")
    selected, deduplication = importer.select_candidates(jackett, prowlarr)
    importer.write_catalog(selected, args.output / "canonical", args.output / "catalog.json",
        len(jackett), len(prowlarr), deduplication, jackett_revision, prowlarr_revision)
    for candidate in jackett + prowlarr:
        # Preserve upstream version directories: same ID across versions is expected, never discarded.
        target = args.output / "variants" / candidate.project / candidate.relative_path
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            raise ValueError(f"duplicate variant path: {target}")
        target.write_bytes(importer.render(importer.convert(candidate)))
        target.with_suffix(".source.txt").write_bytes(candidate.source_bytes)
    (args.output / "revisions.json").write_text(json.dumps({"catalog": revision(args.catalog),
        "jackett": jackett_revision, "prowlarr": prowlarr_revision}, indent=2) + "\n")
    print(f"Prepared {len(selected)} canonical and {len(jackett) + len(prowlarr)} variant definitions")


if __name__ == "__main__":
    main()
