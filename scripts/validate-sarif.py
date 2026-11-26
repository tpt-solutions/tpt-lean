#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Validate `tpt-lean ci --sarif` output against the official OASIS SARIF 2.1.0 JSON schema.

The schema is downloaded on first use into a cache directory (the OASIS copy is not vendored in
this repository). Requires the `jsonschema` package (`pip install jsonschema`).

usage: validate-sarif.py FILE.sarif [--schema PATH_OR_URL]
"""
import argparse
import json
import os
import sys
import urllib.request

SCHEMA_URL = (
    "https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/schemas/"
    "sarif-schema-2.1.0.json"
)


def load_schema(source):
    if source.startswith(("http://", "https://")):
        cache_dir = os.path.join(
            os.environ.get("TMPDIR") or os.environ.get("TEMP") or "/tmp", "tpt-lean-sarif"
        )
        os.makedirs(cache_dir, exist_ok=True)
        cached = os.path.join(cache_dir, os.path.basename(source))
        if not os.path.exists(cached):
            with urllib.request.urlopen(source, timeout=60) as response:
                data = response.read()
            with open(cached, "wb") as f:
                f.write(data)
        source = cached
    with open(source, encoding="utf-8") as f:
        return json.load(f)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("sarif")
    parser.add_argument("--schema", default=SCHEMA_URL)
    args = parser.parse_args()

    import jsonschema

    schema = load_schema(args.schema)
    with open(args.sarif, encoding="utf-8") as f:
        document = json.load(f)
    validator = jsonschema.Draft7Validator(schema) if "draft-07" in schema.get("$schema", "") else None
    if validator is None:
        validator = jsonschema.validators.validator_for(schema)(schema)
    errors = sorted(validator.iter_errors(document), key=lambda e: list(e.path))
    if errors:
        for error in errors[:20]:
            location = "/".join(str(p) for p in error.path) or "(root)"
            print(f"invalid at {location}: {error.message}", file=sys.stderr)
        print(f"{len(errors)} error(s)", file=sys.stderr)
        return 1
    print(f"valid SARIF {document.get('version')} ({args.sarif})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
