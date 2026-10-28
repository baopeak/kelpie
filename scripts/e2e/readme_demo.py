"""Create isolated, runnable repositories for Kelpie README screenshots.

    python scripts/e2e/readme_demo.py <empty-output-folder>

Import ``seed(root)`` to receive eight project dictionaries. The same list is
saved as ``root/projects.json``. This script never reads or changes Kelpie state,
starts agents, contacts remotes, or runs the generated applications/checks.
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import textwrap


# These are demo task labels, not agent activity or results of executed checks.
PROJECTS = (
    ("atlas-commerce", "review", "Review coupon normalization and discounted shipping totals."),
    ("beacon-api", "progress", "Extend the local catalog API with a category filter."),
    ("orbit-mobile", "todo", "Plan an offline sync conflict-resolution screen."),
    ("prism-design", "review", "Review accessible color tokens before publishing the palette."),
    ("harbor-infra", "progress", "Document the staging capacity budget and rollout gate."),
    ("pulse-analytics", "done", "Demo implementation: daily revenue aggregation and CSV export."),
    ("relay-workers", "todo", "Design a retry policy for the deterministic local job queue."),
    ("field-notes", "done", "Demo implementation: searchable Markdown research index."),
)


CHECKOUT = '''\
"""Integer-cent checkout calculations; no network, payment gateway, or credentials."""
from dataclasses import asdict, dataclass
import json


FREE_SHIPPING_CENTS = 10_000
STANDARD_SHIPPING_CENTS = 595
COUPONS = {"WELCOME10": 10, "STUDIO20": 20}


@dataclass(frozen=True)
class LineItem:
    sku: str
    unit_price_cents: int
    quantity: int = 1

    def subtotal(self):
        if not self.sku or self.unit_price_cents < 0 or self.quantity < 1:
            raise ValueError("A line needs a SKU, nonnegative price, and positive quantity")
        return self.unit_price_cents * self.quantity


@dataclass(frozen=True)
class Quote:
    subtotal_cents: int
    discount_cents: int
    shipping_cents: int
    total_cents: int
    coupon: str


def quote(items, coupon=""):
    """Round discounts half-up; free shipping uses the discounted subtotal."""
    items = list(items)
    if not items:
        raise ValueError("The cart must contain at least one item")
    if coupon and coupon not in COUPONS:
        raise ValueError("Unknown coupon code")
    subtotal = sum(item.subtotal() for item in items)
    percent = COUPONS.get(coupon, 0)
    discount = (subtotal * percent + 50) // 100
    discounted = subtotal - discount
    shipping = 0 if discounted >= FREE_SHIPPING_CENTS else STANDARD_SHIPPING_CENTS
    return Quote(subtotal, discount, shipping, discounted + shipping, coupon)


if __name__ == "__main__":
    cart = [LineItem("canvas-tote", 3200, 2), LineItem("field-journal", 1800)]
    print(json.dumps(asdict(quote(cart, "WELCOME10")), indent=2))
'''


APPS = {
    "beacon-api": '''\
"""Local catalog routing, usable directly or via an optional loopback HTTP server."""
import argparse
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

CATALOG = json.loads((Path(__file__).resolve().parents[1] / "data/catalog.json").read_text())


def route(target):
    url = urlsplit(target)
    if url.path == "/health":
        return 200, {"status": "ok", "service": "beacon"}
    if url.path == "/v1/products":
        category = parse_qs(url.query).get("category", [None])[0]
        products = [p for p in CATALOG if category is None or p["category"] == category]
        return 200, {"items": products, "count": len(products)}
    return 404, {"error": "not_found"}


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        status, result = route(self.path)
        body = json.dumps(result).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--serve", action="store_true")
    args = parser.parse_args()
    if args.serve:
        print("Catalog available at http://127.0.0.1:8765", flush=True)
        HTTPServer(("127.0.0.1", 8765), Handler).serve_forever()
    else:
        print(json.dumps(route("/v1/products")[1], indent=2))
''',
    "orbit-mobile": '''\
"""Offline-first note reconciliation with explicit conflict reporting."""
import json


def reconcile(local, incoming):
    merged = {note["id"]: dict(note) for note in local}
    conflicts = []
    for note in incoming:
        previous = merged.get(note["id"])
        if previous and previous["revision"] == note["revision"] and previous["text"] != note["text"]:
            conflicts.append(note["id"])
        elif previous is None or note["revision"] > previous["revision"]:
            merged[note["id"]] = dict(note)
    return {"notes": sorted(merged.values(), key=lambda note: note["id"]),
            "conflicts": sorted(set(conflicts))}


if __name__ == "__main__":
    local = [{"id": "n1", "revision": 2, "text": "Trail closes at dusk"}]
    incoming = [{"id": "n2", "revision": 1, "text": "Pack a rain shell"}]
    print(json.dumps(reconcile(local, incoming), indent=2))
''',
    "prism-design": '''\
"""WCAG contrast calculations for a small, reusable design-token palette."""
import json
from pathlib import Path


def luminance(color):
    color = color.lstrip("#")
    if len(color) != 6:
        raise ValueError("Expected a six-digit RGB hex color")
    channels = [int(color[i:i + 2], 16) / 255 for i in (0, 2, 4)]
    linear = [c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4 for c in channels]
    return sum(c * weight for c, weight in zip(linear, (0.2126, 0.7152, 0.0722)))


def contrast(foreground, background):
    low, high = sorted((luminance(foreground), luminance(background)))
    return (high + 0.05) / (low + 0.05)


if __name__ == "__main__":
    tokens = json.loads((Path(__file__).resolve().parents[1] / "data/tokens.json").read_text())
    for name in ("text", "accent", "muted"):
        ratio = contrast(tokens[name], tokens["background"])
        print(f"{name:8} {tokens[name]}  {ratio:.2f}:1  normal-text AA={ratio >= 4.5}")
''',
    "harbor-infra": '''\
"""Capacity planning arithmetic; does not provision or access infrastructure."""
import json
import math
from pathlib import Path


def capacity(requests_per_second, per_replica=80, utilization=0.65, minimum=2):
    if requests_per_second < 0 or per_replica <= 0 or not 0 < utilization <= 1 or minimum < 1:
        raise ValueError("Invalid capacity inputs")
    replicas = max(minimum, math.ceil(requests_per_second / (per_replica * utilization)))
    return {"replicas": replicas, "cpu_millicores": replicas * 250,
            "memory_mib": replicas * 256, "target_utilization": utilization}


if __name__ == "__main__":
    environments = json.loads((Path(__file__).resolve().parents[1] / "data/environments.json").read_text())
    print(json.dumps({name: capacity(rps) for name, rps in environments.items()}, indent=2))
''',
    "pulse-analytics": '''\
"""Aggregate a synthetic order ledger into a daily revenue report."""
import csv
from pathlib import Path
import sys


def summarize(rows):
    days = {}
    for row in rows:
        if row["status"] != "paid":
            continue
        day = days.setdefault(row["date"], {"orders": 0, "revenue_cents": 0})
        day["orders"] += 1
        day["revenue_cents"] += int(row["amount_cents"])
    return [{"date": date, **totals} for date, totals in sorted(days.items())]


if __name__ == "__main__":
    with (Path(__file__).resolve().parents[1] / "data/orders.csv").open(newline="") as source:
        report = summarize(csv.DictReader(source))
    writer = csv.DictWriter(sys.stdout, fieldnames=("date", "orders", "revenue_cents"), lineterminator="\\n")
    writer.writeheader()
    writer.writerows(report)
''',
    "relay-workers": '''\
"""An in-memory idempotent worker; all job processing is local and synchronous."""
from collections import deque
import json


def process(jobs):
    pending = deque(jobs)
    seen = set()
    results = []
    while pending:
        job = pending.popleft()
        if job["id"] in seen:
            continue
        if job["operation"] != "uppercase":
            raise ValueError("Unsupported local operation")
        results.append({"id": job["id"], "result": job["payload"].upper()})
        seen.add(job["id"])
    return results


if __name__ == "__main__":
    jobs = [{"id": "mail-001", "operation": "uppercase", "payload": "release notes ready"},
            {"id": "mail-002", "operation": "uppercase", "payload": "invoice preview"}]
    print(json.dumps(process(jobs), indent=2))
''',
    "field-notes": '''\
"""Index and search Markdown field notes, without third-party parsers."""
import json
from pathlib import Path
import re
import sys


def index(folder):
    notes = []
    for path in sorted(Path(folder).glob("*.md")):
        text = path.read_text(encoding="utf-8")
        title = next((line[2:].strip() for line in text.splitlines() if line.startswith("# ")), path.stem)
        notes.append({"file": path.name, "title": title,
                      "tags": sorted(set(re.findall(r"(?<!\\w)#([a-z][\\w-]*)", text))),
                      "text": text})
    return notes


def search(notes, query):
    return [note for note in notes if query.casefold() in note["text"].casefold()]


if __name__ == "__main__":
    notes = index(Path(__file__).resolve().parents[1] / "notes")
    matches = search(notes, " ".join(sys.argv[1:]) or "trail")
    print(json.dumps([{key: note[key] for key in ("file", "title", "tags")} for note in matches], indent=2))
''',
}


SMOKES = {
    "atlas-commerce": '''\
from src.checkout import LineItem, quote

cart = [LineItem("canvas-tote", 3200, 2), LineItem("field-journal", 1800)]
result = quote(cart, "WELCOME10")
assert result.subtotal_cents == 8200
assert result.discount_cents == 820
assert result.shipping_cents == 595
assert result.total_cents == 7975
assert quote([LineItem("kit", 10000)]).shipping_cents == 0
assert quote([LineItem("kit", 10000)], "WELCOME10").shipping_cents == 595
assert quote([LineItem("kit", 12500)], "STUDIO20").total_cents == 10000
assert quote([LineItem("pin", 105)], "WELCOME10").discount_cents == 11
for items, coupon in (([], ""), (cart, "EXPIRED"), ([LineItem("bad", -1)], "")):
    try:
        quote(items, coupon)
    except ValueError:
        pass
    else:
        raise AssertionError("Invalid checkout input was accepted")
''',
    "beacon-api": '''\
from src.app import route

assert route("/health") == (200, {"status": "ok", "service": "beacon"})
status, catalog = route("/v1/products")
assert status == 200 and catalog["count"] == 3
assert route("/v1/products?category=paper")[1]["count"] == 2
assert route("/v1/products?category=unknown")[1]["items"] == []
assert route("/missing")[0] == 404
''',
    "orbit-mobile": '''\
from src.app import reconcile

local = [{"id": "n1", "revision": 2, "text": "local"}]
assert reconcile(local, [{"id": "n1", "revision": 1, "text": "old"}])["notes"] == local
assert reconcile(local, [{"id": "n1", "revision": 2, "text": "remote"}])["conflicts"] == ["n1"]
assert reconcile(local, [{"id": "n1", "revision": 3, "text": "new"}])["notes"][0]["text"] == "new"
assert local[0]["text"] == "local"
assert len(reconcile(local, [{"id": "n2", "revision": 1, "text": "new"}])["notes"]) == 2
''',
    "prism-design": '''\
from src.app import contrast, luminance

assert abs(contrast("#000000", "#ffffff") - 21) < 0.0001
assert contrast("#123456", "#123456") == 1
assert contrast("#e7edf6", "#101722") >= 4.5
assert luminance("#000000") == 0
assert abs(luminance("#ffffff") - 1) < 0.0001
''',
    "harbor-infra": '''\
from src.app import capacity

assert capacity(0)["replicas"] == 2
assert capacity(260)["replicas"] == 5
assert capacity(261)["replicas"] == 6
assert capacity(260)["memory_mib"] == 1280
try:
    capacity(1, utilization=0)
except ValueError:
    pass
else:
    raise AssertionError("Zero utilization was accepted")
''',
    "pulse-analytics": '''\
from src.app import summarize

rows = [{"date": "2026-10-20", "amount_cents": "3200", "status": "paid"},
        {"date": "2026-10-20", "amount_cents": "1800", "status": "paid"},
        {"date": "2026-10-20", "amount_cents": "9999", "status": "refunded"}]
assert summarize(rows) == [{"date": "2026-10-20", "orders": 2, "revenue_cents": 5000}]
assert summarize([]) == []
''',
    "relay-workers": '''\
from src.app import process

job = {"id": "job-1", "operation": "uppercase", "payload": "ready"}
assert process([job, job]) == [{"id": "job-1", "result": "READY"}]
assert process([]) == []
assert job["payload"] == "ready"
try:
    process([{**job, "operation": "unknown"}])
except ValueError:
    pass
else:
    raise AssertionError("Unknown operation was accepted")
''',
    "field-notes": '''\
from pathlib import Path
from src.app import index, search

notes = index(Path(__file__).resolve().parent / "notes")
assert len(notes) == 3
assert len(search(notes, "TRAIL")) == 2
assert search(notes, "no-matching-term") == []
assert "research" in next(note for note in notes if note["file"] == "trail-interviews.md")["tags"]
''',
}


DETAILS = {
    "atlas-commerce": ("A thoughtful checkout for independent makers", "Price a cart using integer cents, explicit coupon rules, and a discounted-subtotal shipping threshold.", "python src/checkout.py", "Cart", "Coupon rules", "Checkout quote", "Document normalized coupon input", "Review the free-shipping boundary"),
    "beacon-api": ("A small catalog with a clear API contract", "Serve a local product catalog with category filtering and predictable JSON errors.", "python src/app.py", "HTTP request", "Catalog router", "JSON response", "Document category pagination", "Add cache-control policy"),
    "orbit-mobile": ("Offline notes that travel with you", "Reconcile revisioned notes without overwriting equal-revision conflicts or mutating local input.", "python src/app.py", "Local notes", "Revision merge", "Conflict list", "Sketch the conflict-resolution screen", "Specify tombstone retention"),
    "prism-design": ("A palette built for readable interfaces", "Calculate WCAG luminance and contrast for a compact set of dark-surface design tokens.", "python src/app.py", "Color tokens", "Contrast calculator", "Accessibility report", "Review muted-text contrast", "Document large-text usage"),
    "harbor-infra": ("Capacity budgets before cloud resources", "Estimate replica, CPU, and memory budgets using explicit utilization targets; no cloud account needed.", "python src/app.py", "Traffic forecast", "Capacity planner", "Resource budget", "Review staging traffic assumptions", "Define a rollout approval gate"),
    "pulse-analytics": ("Daily revenue, without spreadsheet archaeology", "Turn a synthetic order ledger into a daily CSV report while excluding non-paid orders.", "python src/app.py", "Order ledger", "Daily aggregation", "Revenue CSV", "Aggregate paid orders by day", "Document the report columns"),
    "relay-workers": ("Small jobs, predictable delivery", "Process a local queue in FIFO order, suppress duplicate job IDs, and reject unsupported operations.", "python src/app.py", "Job queue", "Idempotent worker", "Result ledger", "Specify bounded retry delays", "Define dead-letter retention"),
    "field-notes": ("A searchable home for research observations", "Index Markdown titles and tags, then search a tiny collection of synthetic field research notes.", "python src/app.py trail", "Markdown notes", "Title and tag index", "Search results", "Index research notes and tags", "Document case-insensitive search"),
}


def _put(root, relative, content):
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(content), encoding="utf-8", newline="\n")


def _git(repo, *args, date="2026-10-20T09:00:00+00:00"):
    # Inherited GIT_DIR / GIT_WORK_TREE must never redirect writes to another repo.
    environment = {key: value for key, value in os.environ.items()
                   if not key.upper().startswith("GIT_")}
    environment.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                       GIT_TERMINAL_PROMPT="0", GIT_AUTHOR_DATE=date,
                       GIT_COMMITTER_DATE=date)
    return subprocess.run(["git", "-C", str(repo), *args], env=environment,
                          check=True, capture_output=True, text=True).stdout


def _assets(repo, name):
    if name == "atlas-commerce":
        _put(repo, "data/catalog.json", json.dumps([
            {"sku": "canvas-tote", "price_cents": 3200, "stock": 42},
            {"sku": "field-journal", "price_cents": 1800, "stock": 85},
            {"sku": "studio-kit", "price_cents": 12500, "stock": 12},
        ], indent=2) + "\n")
        _put(repo, "docs/checkout-rules.md", """\
            # Checkout rules

            All prices and totals are integer cents. Discount rounding is half-up.

            | Code | Discount | Intended use |
            | --- | ---: | --- |
            | WELCOME10 | 10% | First studio order |
            | STUDIO20 | 20% | Seasonal maker bundle |

            Shipping is 595 cents, waived when the **discounted** subtotal is at
            least 10,000 cents. A 100-dollar cart with WELCOME10 still pays shipping.
            Unknown coupons and empty carts are errors, not silent fallbacks.

            ## Review focus

            The main working copy adds whitespace trimming and case normalization
            before coupon lookup. This is an uncommitted demo change for diff review.
            No payments are accepted and no customer data is stored.
            """)
    elif name == "beacon-api":
        _put(repo, "data/catalog.json", json.dumps([
            {"id": "journal", "name": "Field Journal", "category": "paper", "price_cents": 1800},
            {"id": "cards", "name": "Trail Cards", "category": "paper", "price_cents": 900},
            {"id": "tote", "name": "Canvas Tote", "category": "accessories", "price_cents": 3200},
        ], indent=2) + "\n")
    elif name == "prism-design":
        _put(repo, "data/tokens.json", json.dumps({"background": "#101722", "text": "#e7edf6",
                                                 "accent": "#83d6c4", "muted": "#9aa9bc"}, indent=2) + "\n")
    elif name == "harbor-infra":
        _put(repo, "data/environments.json", '{"development": 12, "staging": 85, "production": 260}\n')
    elif name == "pulse-analytics":
        _put(repo, "data/orders.csv", "date,order_id,amount_cents,status\n2026-10-20,A101,3200,paid\n2026-10-20,A102,1800,paid\n2026-10-20,A103,900,refunded\n2026-10-21,A104,12500,paid\n2026-10-21,A105,6400,paid\n2026-10-22,A106,1800,pending\n2026-10-22,A107,4100,paid\n")
    elif name == "field-notes":
        _put(repo, "notes/trail-interviews.md", "# Trail interviews\n\n#research #offline\n\nSynthetic observation: hikers need offline trail notes before leaving the station.\n")
        _put(repo, "notes/packing-study.md", "# Packing study\n\n#research #planning\n\nSynthetic observation: a short packing checklist reduces repeated gear checks.\n")
        _put(repo, "notes/wayfinding.md", "# Wayfinding\n\n#navigation #offline\n\nSynthetic observation: trail markers should remain readable in low contrast.\n")


def seed(root):
    """Seed eight Git projects in a new/empty root and return board metadata.

    Raises FileExistsError for any nonempty root; never deletes existing content.
    Every generated file, Git repository, and linked worktree stays beneath root.
    On failure partial output is retained for inspection, never silently replaced.
    """
    root = Path(root).expanduser().absolute()
    if root.is_symlink():
        raise ValueError("The output root must not be a symbolic link")
    if root.exists() and (not root.is_dir() or any(root.iterdir())):
        raise FileExistsError(f"Refusing nonempty output root: {root}")
    root.mkdir(parents=True, exist_ok=True)
    projects = []
    for number, (name, status, comment) in enumerate(PROJECTS):
        repo = root / name
        repo.mkdir()
        _git(repo, "init", "--template=", "-q", "-b", "main")
        _git(repo, "config", "user.name", "Kelpie Demo")
        _git(repo, "config", "user.email", "demo@kelpie.invalid")
        _git(repo, "config", "commit.gpgSign", "false")
        _git(repo, "config", "core.autocrlf", "false")
        # Disable user hooks, including when a generated worktree is used later.
        hooks = repo / ".git" / "hooks"
        hooks.mkdir(exist_ok=True)
        _git(repo, "config", "core.hooksPath", str(hooks))
        title, description, command, source, behavior, result, task1, task2 = DETAILS[name]
        _put(repo, ".gitignore", "__pycache__/\n*.py[cod]\n.venv/\n")
        _put(repo, ".kelpie.yaml", "# Local, dependency-free demo check; explicit approval is still required.\ncheck:\n  - python smoke.py\n")
        _put(repo, "README.md", f"""\
            # {name}

            ## {title}

            {description}

            > A self-contained Kelpie demo project. All records and latency values
            > are synthetic; no production services, credentials, or agents are used.

            ## Run locally

            Python 3.10+ and Git are the only requirements. From this project folder:

            ```sh
            {command}
            python smoke.py
            ```

            The default application command terminates after printing its result.
            {"Optional local server: `python src/app.py --serve` (loopback port 8765)." if name == "beacon-api" else "No network access is required."}

            ## Explore the project

            | File | Purpose |
            | --- | --- |
            | `src/{'checkout' if name == 'atlas-commerce' else 'app'}.py` | Runnable domain behavior |
            | `smoke.py` | Executable assertions for the core contract |
            | `docs/architecture.md` | Data flow and design boundaries |
            | `data/latency.csv` | Synthetic timing data for the CSV viewer |
            | `specs/release/tasks.md` | Release checklist for the spec panel |

            ## Design boundaries

            - Keep inputs local and output deterministic.
            - Prefer transparent rules over hidden dependencies.
            - Treat this checklist as demo planning, not agent execution history.
            """)
        _put(repo, "src/__init__.py", '"""Local demo domain code."""\n')
        _put(repo, "src/checkout.py" if name == "atlas-commerce" else "src/app.py",
             CHECKOUT if name == "atlas-commerce" else APPS[name])
        _assets(repo, name)
        _git(repo, "add", ".")
        _git(repo, "commit", "-q", "-m", f"feat: implement {name} local demo")

        _put(repo, "smoke.py", '"""Run local contract assertions; no external services or dependencies."""\n'
             + SMOKES[name] + f'\nprint("PASS {name}: local contract assertions")\n')
        _put(repo, "docs/architecture.md", f"""\
            # {name}: architecture

            ## Data flow

            ```mermaid
            flowchart LR
                input["{source}"] --> domain["{behavior}"]
                domain --> output["{result}"]
                checks["Smoke assertions"] -. verify .-> domain
            ```

            ## Responsibility boundaries

            **Input:** {source}. Demo data is synthetic and checked into this repository.

            **Domain:** {description}

            **Output:** {result}. The default command prints locally and exits.

            ## Trade-offs

            The miniature implementation is intentionally in-memory and single-process.
            It is useful for understanding the domain rules, not a production deployment.
            Smoke assertions encode representative edge cases without requiring a test runner.

            ## Timing dataset

            `data/latency.csv` contains constructed values for demonstrating CSV sorting
            and inspection. It is not a benchmark or a report of executed requests.
            """)
        rows = ["date,operation,samples,p50_ms,p95_ms,error_count"]
        operation = {"atlas-commerce": "checkout.quote", "beacon-api": "catalog.list",
                     "orbit-mobile": "notes.reconcile", "prism-design": "palette.contrast",
                     "harbor-infra": "capacity.plan", "pulse-analytics": "revenue.aggregate",
                     "relay-workers": "queue.process", "field-notes": "notes.search"}[name]
        for day in range(14, 25):
            p50 = 8 + number * 3 + (day % 4)
            rows.append(f"2026-10-{day},{operation},{120 + day * 7},{p50},{p50 * 2 + day % 3},0")
        _put(repo, "data/latency.csv", "\n".join(rows) + "\n")
        complete = "x" if status == "done" else " "
        _put(repo, "specs/release/tasks.md", f"""\
            # {name}: release checklist

            These are demo planning tasks, not evidence of an agent run.
            Checked items describe code or documentation present in this fixture;
            the generator does not execute checks.

            ## Implemented

            - [x] Implement the local domain behavior
            - [x] Add representative smoke assertions
            - [x] Document the data flow with a Mermaid diagram

            ## Release work

            - [{complete}] {task1}
            - [{complete}] {task2}
            - [ ] Run `python smoke.py` in the target environment

            ## Acceptance notes

            {comment}

            Keep sample data synthetic and do not add remote services or credentials.
            """)
        _git(repo, "add", ".")
        _git(repo, "commit", "-q", "-m", "docs: add release checklist, architecture and smoke contracts",
             date="2026-10-21T14:30:00+00:00")
        projects.append({"name": name, "path": str(repo), "status": status, "comment": comment})

    atlas = root / "atlas-commerce"
    for branch, folder in (("feature/coupon-validation", "coupon-validation"),
                           ("fix/shipping-threshold", "shipping-threshold")):
        worktree = root / "worktrees" / folder
        worktree.parent.mkdir(parents=True, exist_ok=True)
        _git(atlas, "worktree", "add", "-q", "-b", branch, str(worktree), "main")
    # Only the main checkout has an uncommitted code change. Both worktrees start
    # from the second commit, so their baseline smoke contracts remain identical.
    _put(atlas, "src/checkout.py", CHECKOUT.replace(
        '    if coupon and coupon not in COUPONS:\n',
        '    # Accept codes pasted from emails without changing discount rules.\n'
        '    coupon = coupon.strip().upper()\n'
        '    if coupon and coupon not in COUPONS:\n'))
    _put(root, "projects.json", json.dumps(projects, indent=2) + "\n")
    return projects


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", help="New or empty folder; all fixture data stays here")
    args = parser.parse_args()
    try:
        projects = seed(args.root)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        detail = error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) else str(error)
        parser.exit(1, f"readme_demo: {detail}\n")
    print(json.dumps(projects, indent=2))


if __name__ == "__main__":
    main()
