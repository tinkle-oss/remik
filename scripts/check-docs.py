#!/usr/bin/env python3
"""Check repository-local Markdown link targets without fetching external URLs."""
import re
from pathlib import Path
from urllib.parse import unquote, urlsplit


def broken_links(root):
    documents = [*root.glob("*.md"), *(root / "docs").rglob("*.md"), *(root / ".github").rglob("*.md")]
    broken = []
    for document in documents:
        for target in re.findall(r"\[[^\]]*\]\(([^\s)]+)\)", document.read_text()):
            url = urlsplit(target)
            if url.scheme or url.netloc or not url.path:
                continue
            path = (document.parent / unquote(url.path)).resolve()
            if not path.is_relative_to(root.resolve()) or not path.exists():
                broken.append(f"{document.relative_to(root)}: {target}")
    return broken


if __name__ == "__main__":
    failures = broken_links(Path(__file__).resolve().parent.parent)
    if failures:
        raise SystemExit("Broken documentation links:\n" + "\n".join(failures))
    print("Repository-local documentation links passed")
