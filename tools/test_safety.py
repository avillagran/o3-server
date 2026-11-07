"""Reject explicit recursive test cleanup without treating inert Rust strings as code."""
from pathlib import Path
import re

FORBIDDEN = {"remove_dir_all", "rmtree"}
RAW = re.compile(r'(?:br|r)(#*)"')
CHAR = re.compile(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|.)|[^'\\\n])'")
WORD = re.compile(r"[a-zA-Z_][a-zA-Z_0-9]*")


def positive_test_attribute(attributes):
    """Find test inside cfg expressions, accounting for nested not(...) groups."""
    negations = [False]
    previous = None
    for value in attributes:
        if value == "(":
            negations.append(negations[-1] ^ (previous == "not"))
        elif value == ")":
            if len(negations) > 1:
                negations.pop()
        elif value == "test" and not negations[-1]:
            return True
        previous = value
    return False


def tokens(source):
    result = []
    index = 0
    line = 1
    while index < len(source):
        start = index
        if source.startswith("//", index):
            index = source.find("\n", index)
            if index < 0:
                break
        elif source.startswith("/*", index):
            index += 2
            depth = 1
            while index < len(source) and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
        elif raw := RAW.match(source, index):
            marker = '"' + raw.group(1)
            end = source.find(marker, raw.end())
            index = len(source) if end < 0 else end + len(marker)
        elif source[index] == '"':
            index += 1
            while index < len(source):
                if source[index] == "\\":
                    index += 2
                elif source[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
        elif char := CHAR.match(source, index):
            index = char.end()
        elif word := WORD.match(source, index):
            result.append((word.group(), line))
            index = word.end()
        else:
            if not source[index].isspace():
                result.append((source[index], line))
            index += 1
        line += source[start:index].count("\n")
    return result


def recursive_calls(source, integration=False):
    stream = tokens(source)
    scopes = [integration]
    pending = False
    item = False
    result = []
    index = 0
    while index < len(stream):
        token, line = stream[index]
        if token == "#" and index + 1 < len(stream) and stream[index + 1][0] == "[":
            depth = 1
            end = index + 2
            attributes = []
            while end < len(stream) and depth:
                value = stream[end][0]
                if value == "[":
                    depth += 1
                elif value == "]":
                    depth -= 1
                if depth:
                    attributes.append(value)
                end += 1
            prefix = attributes[:attributes.index("(")] if "(" in attributes else attributes
            test_attribute = prefix == ["test"] or (
                bool(prefix) and prefix[-1] == "test" and ":" in prefix
            )
            test_cfg = attributes[:2] == ["cfg", "("] and positive_test_attribute(attributes)
            test_cfg = test_cfg or attributes[:2] == ["cfg_attr", "("] and positive_test_attribute(attributes)
            pending = pending or test_attribute or test_cfg
            index = end
            continue
        if token in {"fn", "mod", "impl", "struct", "enum", "trait", "const", "static"}:
            item = True
        if token == "{":
            scopes.append(scopes[-1] or pending)
            pending = False
            item = False
        elif token == "}":
            if len(scopes) > 1:
                scopes.pop()
            pending = False
            item = False
        elif token == ";" or token == "," and not item:
            pending = False
            item = False
        elif token in FORBIDDEN and (scopes[-1] or pending):
            result.append((line, token))
        index += 1
    return result


def violations(root):
    findings = []
    for folder in (root / "src", root / "tests"):
        for path in sorted(folder.rglob("*.rs")):
            if path.is_symlink():
                findings.append(f"{path.relative_to(root)}: symbolic source requires explicit review")
                continue
            integration = path.is_relative_to(root / "tests") or path.stem.endswith(("_tests", "_test"))
            for line, operation in recursive_calls(path.read_text(), integration):
                findings.append(f"{path.relative_to(root)}:{line}: forbidden test operation {operation}")
    return findings


def check(root, full=False):
    findings = violations(root)
    if full and (root / "tests/SAFETY-AUDIT-PENDING").exists():
        findings.append("Full runtime matrix is blocked until the production-operation fixture audit is complete.")
    if findings:
        raise SystemExit("\n".join(findings))
