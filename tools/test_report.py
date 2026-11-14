"""Runs the automated tests and writes docs/TEST-REPORT.md.

    python tools/test_report.py

Run it for every build, before committing. The part of the report between the two "by hand"
markers is written by people (and kept as it is); everything else is replaced by this run's
results. The script ends with an error when a test fails, after writing the report.
"""
import datetime
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPORT = os.path.join(ROOT, "docs", "TEST-REPORT.md")
START, END = "<!-- by hand: start -->", "<!-- by hand: end -->"

# What each group of tests is, by the name cargo prints for it.
SUITES = [
    (r"unittests src\\lib\.rs \(target\\release\\deps\\rogeha_core", "Engine, small parts", "Dates, signature encoding, font names (crates/core, unit tests)"),
    (r"tests\\page_ops\.rs", "Engine, on real documents", "Opens, edits, saves and reopens PDFs made for the test (crates/core/tests/page_ops.rs)"),
    (r"unittests src\\main\.rs \(target\\release\\deps\\rogeha_reader", "Reader", "Page store, printing, dark pages, the assistant's citations, page layout (crates/app, unit tests)"),
    (r"unittests src\\main\.rs \(target\\release\\deps\\rogeha_agent", "Assistant", "The assistant's protocol and settings (crates/agent, unit tests)"),
    (r"Doc-tests", "Examples in the documentation", "Code examples in doc comments"),
]

BY_HAND_TEMPLATE = """## Checked by scripted runs and by hand

Nothing recorded yet.
"""


def git(*args):
    return subprocess.run(["git", "-C", ROOT, *args], capture_output=True, text=True).stdout.strip()


def version():
    cargo = open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8").read()
    major, minor = re.search(r'^version\s*=\s*"(\d+)\.(\d+)\.', cargo, re.M).groups()
    return f"{major}.{minor}.{git('rev-list', '--count', 'HEAD')}"


def sentence(name):
    """a_test_name -> A test name"""
    words = name.split("::")[-1].replace("_", " ")
    return words[:1].upper() + words[1:]


def main():
    print("Running cargo test --release ...")
    run = subprocess.run(["cargo", "test", "--release"], cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace")
    output = run.stdout + "\n" + run.stderr

    # cargo prints "Running <what>" on stderr and each suite's tests on stdout, in the same order.
    headers = re.findall(r"^\s*(?:Running|Doc-tests) (.+)$", run.stderr, re.M)
    doc_tests = re.findall(r"^\s*Doc-tests (.+)$", run.stderr, re.M)
    blocks = re.split(r"^running \d+ tests?$", run.stdout, flags=re.M)[1:]
    names = re.findall(r"^\s*Running (.+)$", run.stderr, re.M) + [f"Doc-tests {d}" for d in doc_tests]
    suites = []
    for name, block in zip(names, blocks):
        tests = re.findall(r"^test (.+?) \.\.\. (ok|FAILED|ignored)", block, re.M)
        title, about = next(((t, a) for pattern, t, a in SUITES if re.search(pattern, name)), (name, ""))
        suites.append((title, about, tests))

    total = sum(len(t) for _, _, t in suites)
    failed = [(s, n) for s, _, tests in suites for n, r in tests if r == "FAILED"]
    ignored = sum(1 for _, _, tests in suites for _, r in tests if r == "ignored")
    passed = total - len(failed) - ignored
    compiled = run.returncode == 0 or bool(blocks)

    dirty = bool(git("status", "--porcelain", "--", ".", ":!docs/TEST-REPORT.md"))
    commit = "`" + git("rev-parse", "--short", "HEAD") + "`" + (", plus changes not yet committed" if dirty else "")
    today = datetime.date.today().strftime("%d %B %Y").lstrip("0")

    by_hand = BY_HAND_TEMPLATE
    if os.path.exists(REPORT):
        old = open(REPORT, encoding="utf-8").read()
        if START in old and END in old:
            by_hand = old.split(START, 1)[1].split(END, 1)[0].strip("\n") + "\n"

    lines = [
        "# Rogeha Reader: test report",
        "",
        "What was tested for this build, and what was not. The automated part is written by",
        "`python tools/test_report.py`, which runs the tests; run it for every build. The part on scripted runs",
        "and checks by hand is kept by people: change it when such a check is done or a known gap closes.",
        "",
        "| | |",
        "| --- | --- |",
        f"| Version tested | {version()} (commit {commit}) |",
        f"| Date | {today} |",
        "| Command | `cargo test --release` |",
    ]
    if not compiled:
        lines += ["| Result | **The tests did not build.** See the end of this report |"]
    elif failed:
        lines += [f"| Result | **{len(failed)} of {total} tests failed**, {passed} passed |"]
    else:
        lines += [f"| Result | **All {total} automated tests passed**" + (f" ({ignored} skipped)" if ignored else "") + " |"]
    lines += [
        "",
        "The version above is the build the tests ran on. Committing this report adds one to the build number.",
        "",
        "## Automated tests",
        "",
        "| Group | Tests | Passed | Failed |",
        "| --- | --- | --- | --- |",
    ]
    for title, _, tests in suites:
        if tests:
            bad = sum(1 for _, r in tests if r == "FAILED")
            lines.append(f"| {title} | {len(tests)} | {sum(1 for _, r in tests if r == 'ok')} | {bad} |")
    lines.append(f"| **All** | **{total}** | **{passed}** | **{len(failed)}** |")
    lines.append("")
    if failed:
        lines += ["### Failed", ""] + [f"- **{sentence(n)}** ({s}; `{n}`)" for s, n in failed] + [""]
    for title, about, tests in suites:
        if not tests:
            continue
        lines += [f"### {title}", "", about + ".", "", "| Test | Result |", "| --- | --- |"]
        for name, result in sorted(tests):
            shown = {"ok": "passed", "FAILED": "**failed**", "ignored": "skipped"}[result]
            lines.append(f"| {sentence(name)} | {shown} |")
        lines.append("")
    lines += [START, by_hand.rstrip("\n"), END, ""]
    if not compiled:
        lines += ["## Build output", "", "```", output[-3000:].strip(), "```", ""]

    os.makedirs(os.path.dirname(REPORT), exist_ok=True)
    with open(REPORT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines))
    print(f"{REPORT}: {passed} passed, {len(failed)} failed, {ignored} skipped, of {total}")
    if failed or not compiled:
        sys.exit(1)


if __name__ == "__main__":
    main()
