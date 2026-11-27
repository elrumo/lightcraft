#!/usr/bin/env python3
"""Generate server release notes from committed Git subjects through LiteLLM."""

import argparse
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
ENV_FILE = ROOT / "apps/lightbites-web/.env"


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def configuration():
    values = {}
    if ENV_FILE.is_file():
        for line in ENV_FILE.read_text().splitlines():
            match = re.match(r"\s*(?:export\s+)?(LITELLM_API_BASE_URL|LITELLM_API_KEY)\s*=\s*(.*)", line)
            if match:
                parts = shlex.split(match.group(2), comments=True)
                values[match.group(1)] = parts[0] if parts else ""
    for name in ("LITELLM_API_BASE_URL", "LITELLM_API_KEY"):
        if name in os.environ:
            values[name] = os.environ[name]
    return values


def generate(version):
    base = os.environ.get("RELEASE_NOTES_BASE")
    if not base:
        tags = git("tag", "--merged", "HEAD", "--list", "server-v*", "--sort=-version:refname").splitlines()
        base = next((tag for tag in tags if tag != f"server-v{version}"), None)
    if not base:
        raise ValueError("No previous server tag found; set RELEASE_NOTES_BASE to a local Git ref")
    git("rev-parse", "--verify", f"{base}^{{commit}}")
    if subprocess.run(["git", "merge-base", "--is-ancestor", base, "HEAD"], cwd=ROOT).returncode:
        raise ValueError("RELEASE_NOTES_BASE must be an ancestor of HEAD")
    # Subjects only: don't transmit commit bodies, authors, file contents or uncommitted changes.
    subjects = git("log", "--no-merges", "--format=%s", f"{base}..HEAD", "--",
                   "apps/lightcraft-server", "crates/apphost", "crates/catalog", "docs/self-hosting.md")
    if not subjects:
        raise ValueError("No server-related commits found since the baseline")
    if len(subjects) > 60000:
        raise ValueError("Too many commit subjects; choose a more recent RELEASE_NOTES_BASE")
    config = configuration()
    base_url = config.get("LITELLM_API_BASE_URL", "").rstrip("/")
    if not base_url:
        raise ValueError("Set LITELLM_API_BASE_URL in the environment or apps/lightbites-web/.env")
    if not base_url.endswith("/v1"):
        base_url += "/v1"
    payload = {"model": "mimo-v2.6-pro", "messages": [
        {"role": "system", "content": "Write concise Markdown release notes for LightBites Server users. Use only the supplied commit subjects as evidence. Include only changes relevant to server users. Do not invent changes or include private details, commit identifiers, or claims of testing. If a subject is unclear, omit it."},
        {"role": "user", "content": f"Release {version}. Commit subjects since {base}:\n{subjects}"},
    ]}
    headers = {"Content-Type": "application/json"}
    if config.get("LITELLM_API_KEY"):
        headers["Authorization"] = "Bearer " + config["LITELLM_API_KEY"]
    request = urllib.request.Request(base_url + "/chat/completions", json.dumps(payload).encode(), headers)
    try:
        with urllib.request.urlopen(request, timeout=180) as response:
            result = json.load(response)
    except urllib.error.HTTPError as exc:
        raise ValueError(f"LiteLLM request failed with HTTP {exc.code}") from None
    except urllib.error.URLError:
        raise ValueError("Could not reach the LiteLLM gateway") from None
    try:
        notes = result["choices"][0]["message"]["content"]
    except (KeyError, IndexError, TypeError):
        raise ValueError("LiteLLM returned an invalid response") from None
    if not isinstance(notes, str) or not notes.strip():
        raise ValueError("LiteLLM returned empty release notes")
    return notes.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    try:
        print(generate(args.version))
    except (ValueError, OSError, subprocess.CalledProcessError, json.JSONDecodeError) as exc:
        print(f"server-release-notes: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
