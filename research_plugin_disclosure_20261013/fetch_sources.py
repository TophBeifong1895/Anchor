"""Fetch public research sources and retain their original bytes and readable text."""

from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from hashlib import sha256
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import sys
from urllib.request import Request, urlopen


class VisibleText(HTMLParser):
    def __init__(self):
        super().__init__()
        self.skipped = 0
        self.parts = []

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style", "svg"):
            self.skipped += 1

    def handle_endtag(self, tag):
        if tag in ("script", "style", "svg"):
            self.skipped = max(0, self.skipped - 1)

    def handle_data(self, data):
        if not self.skipped and data.strip():
            self.parts.append(data.strip())


def fetch(source):
    label, url = source.split("=", 1)
    if not re.fullmatch(r"[a-z0-9_-]+", label):
        raise ValueError("source label must be a simple filename")
    root = Path(__file__).resolve().parent / "sources"
    root.mkdir(exist_ok=True)
    record = {"url": url, "accessed_at": datetime.now(timezone.utc).isoformat()}
    try:
        request = Request(url, headers={"User-Agent": "Mozilla/5.0 (Anchor source audit)"})
        with urlopen(request, timeout=20) as response:
            body = response.read(8 * 1024 * 1024 + 1)
            if len(body) > 8 * 1024 * 1024:
                raise ValueError("source exceeds the research download limit")
            record.update(status=response.status, effective_url=response.url,
                          content_type=response.headers.get("Content-Type"),
                          sha256=sha256(body).hexdigest(), bytes=len(body))
        (root / (label + ".raw")).write_bytes(body)
        decoded = body.decode("utf-8", errors="replace")
        if "text/html" in (record["content_type"] or ""):
            parser = VisibleText()
            parser.feed(decoded)
            decoded = "\n".join(parser.parts)
        (root / (label + ".txt")).write_text(decoded)
        record["text_file"] = str(root / (label + ".txt"))
    except Exception as error:
        record["error"] = str(error)
    (root / (label + ".json")).write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n")
    return record


if __name__ == "__main__":
    with ThreadPoolExecutor(max_workers=5) as pool:
        for result in pool.map(fetch, sys.argv[1:]):
            print(json.dumps(result, ensure_ascii=False))
