#!/usr/bin/env python3
"""Check the project site (docs/) for broken relative links. Standard library only.

    scripts/check-docs-links.py [docs/index.html ...]

Every relative href/src in the HTML files, every url() in their CSS, and
every #fragment must resolve. External links (http, https, mailto) are not
fetched. Exits with status 1 if anything is missing.
"""
import html.parser
import pathlib
import re
import sys
import urllib.parse

root = pathlib.Path(__file__).resolve().parent.parent / "docs"


class Collect(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.links, self.ids, self.styles = [], set(), []

    def handle_starttag(self, tag, attrs):
        for name, value in attrs:
            if value is None:
                continue
            if name in ("href", "src"):
                self.links.append(value)
            elif name == "id":
                self.ids.add(value)
            elif name == "style":
                self.styles.append(value)


def css_urls(text):
    return [u.strip("'\"") for u in re.findall(r"url\(([^)]+)\)", text)]


def check(page):
    errors = []
    parser = Collect()
    parser.feed(page.read_text(encoding="utf-8"))
    links = [(page, link) for link in parser.links]
    links += [(page, u) for style in parser.styles for u in css_urls(style)]
    for link in parser.links:
        if link.endswith(".css") and not urllib.parse.urlparse(link).scheme:
            css = (page.parent / link).resolve()
            if css.exists():
                links += [(css, u) for u in css_urls(css.read_text(encoding="utf-8"))]
    for base, link in links:
        parsed = urllib.parse.urlparse(link)
        if parsed.scheme in ("http", "https", "mailto", "data"):
            continue
        if not parsed.path:
            if parsed.fragment and parsed.fragment not in parser.ids:
                errors.append(f"{page.name}: no element with id '{parsed.fragment}'")
            continue
        target = (base.parent / urllib.parse.unquote(parsed.path)).resolve()
        if not target.exists():
            errors.append(f"{base.relative_to(root.parent)}: missing {link}")
        elif root not in target.parents and target != root:
            errors.append(f"{base.relative_to(root.parent)}: {link} leaves docs/ (GitHub Pages serves only docs/)")
    return errors, len(links)


def main():
    pages = [pathlib.Path(p).resolve() for p in sys.argv[1:]] or sorted(root.glob("*.html"))
    errors, count = [], 0
    for page in pages:
        e, n = check(page)
        errors += e
        count += n
    for e in errors:
        print(e)
    print(f"{count} links checked in {len(pages)} page(s), {len(errors)} broken")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
