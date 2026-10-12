#!/usr/bin/env python3
"""Refuse raw file deletion added to product code.

Deleting the wrong thing is the failure class review keeps finding: uninstall
through a linked data folder or a case-variant HOME, Browser host retirement
through linked parents and by name prefix, share copy through swapped links,
TURN retirement unlinking through a linked parent. Every deletion added to
product code calls the owned-removal helper of its language, or names in a
comment on the same line (or on the comment line directly above) why the raw
call cannot reach anything else:

    # removal-ok: <reason>        // removal-ok: <reason>

Helpers: remove one exact path below an owned root, never through a link,
optionally only when the leaf still has the device+inode the caller recorded.
    Rust    elastos_common::owned_removal::remove_owned(root, relative, expected)
    Python  scripts/lib/owned_removal.py  remove_owned(root, relative, expected)
    shell   python3 scripts/lib/owned_removal.py ROOT RELATIVE [--expect DEV:INO]
    Node    scripts/lib/owned-removal.mjs removeOwned(root, relative, expected)

Each language is split into code, string literals and comments (a light
lexer, not a parser), so a deletion named in a message or comment passes and a
call after a comment does not. Import aliases of the deletion functions and
rm spawned as a program count. Shell counts rm/rmdir/unlink in command
position (after assignments and wrappers such as sudo or xargs), find -delete,
and commands inside trap/eval/sh -c strings, substitutions and Python or Node
heredocs. This catches accidental raw deletion, not deliberately hidden code.

Only lines added in BASE...HEAD are checked, so existing code does not flood
the gate. Tests, fixtures, vendored code and the helpers themselves are out
of scope.

    removal-guard.py [--base REF] [--head REF]   check added lines (default origin/develop...HEAD)
    removal-guard.py --inventory [--head REF]    list every raw deletion in product code
"""
import argparse
import codecs
from pathlib import PurePosixPath
import re
import subprocess
import sys

# The helpers and this gate hold raw primitives by design.
EXEMPT_FILES = {
    "elastos/crates/elastos-common/src/owned_removal.rs",
    "scripts/lib/owned_removal.py",
    "scripts/lib/owned-removal.mjs",
    "scripts/removal-guard.py",
    "scripts/removal-guard-test.py",
}
# Not product code: docs, CI, vendored and example trees, tests and fixtures.
EXEMPT_DIRS = {".github", ".claude", ".cursor", ".githooks", "docs", "examples", "third-party",
               "third_party", "vendor", "node_modules", "tests", "test", "__tests__", "fixtures",
               "testdata", "benches"}
TEST_NAME = re.compile(r"(?:^tests?[._]|^test_|[._-]tests?\.|[._-]smoke\.|\.integration\.|[-_]repro\.|\.min\.js$)")
LANGUAGES = {".rs": "rust", ".py": "python", ".sh": "shell", ".bash": "shell", ".mjs": "node",
             ".cjs": "node", ".js": "node", ".ts": "node", ".mts": "node", ".cts": "node"}
HELPER = {
    "rust": "elastos_common::owned_removal::remove_owned(root, relative, expected)",
    "python": "remove_owned(root, relative, expected) from scripts/lib/owned_removal.py",
    "shell": 'python3 "${ROOT}/scripts/lib/owned_removal.py" ROOT RELATIVE [--expect DEV:INO]'
             " (embedded Python: remove_owned from scripts/lib/owned_removal.py)",
    "node": "removeOwned(root, relative, expected) from scripts/lib/owned-removal.mjs",
}
MARKER = re.compile(r"removal-ok:\s*\S.{7,}")
RUST_TEST_ATTRIBUTE = re.compile(r"^(\s*)#\[(?:cfg\((?:test|all\([^)]*\btest\b[^)]*\))\)|(?:tokio::)?test\b[^\]]*)\]")

# ── Code languages: calls on code with every string literal as \0N\0 ──────
NAMES = {
    "python": ("remove", "unlink", "rmdir", "removedirs", "rmtree"),
    "rust": ("remove_file", "remove_dir_all", "remove_dir", "unlinkat", "unlink", "rmdir"),
    "node": ("rm", "rmSync", "unlink", "unlinkSync", "rmdir", "rmdirSync"),
}
CALLS = {
    "python": re.compile(r"(?<![\w.])(?:os\.(?:remove|removedirs)|shutil\.rmtree)\s*\(|\.(?:unlink|rmdir|rmtree)\s*\("),
    "rust": re.compile(r"(?<![\w.])(?<!fn )(?:remove_file|remove_dir_all|remove_dir|unlinkat|unlink|rmdir)\s*\("),
    "node": re.compile(r"(?<![\w$])(?<!function )(?:rm|rmSync|unlink|unlinkSync|rmdir|rmdirSync)\s*\("),
}
PROGRAM = re.compile(r"(?:/usr)?(?:/bin/)?(?:rm|rmdir|unlink)")
SPAWN = re.compile(r"(?:\b(?:system|popen|run|call|check_call|check_output|Popen|getoutput|getstatusoutput|"
                   r"spawn|spawnSync|execFile|execFileSync|exec|execSync|execa|execvp?e?|execlp?)|Command::new)"
                   r"\s*\(\s*\[?\s*\0(\d+)\0")
ARGV = re.compile(r"\b\w+\s*=\s*\[\s*\0(\d+)\0")
NODE_DEFINITION = (re.compile(r"\s*(?:(?:async|static|get|set)\s+)*"), re.compile(r"[^()]*\)\s*\{"))

STARTS = {
    "python": re.compile(r"#|(?<![\w])(?:[rRbBuUfF]{1,2})?(?:'''|\"\"\"|'|\")"),
    "rust": re.compile(r"//|/\*|(?<![\w])b?r(#*)\"|(?<![\w])[bc]?\"|'"),
    "node": re.compile(r"//|/\*|['\"`/]"),
}
RUST_CHAR = re.compile(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}|.)|[^\\'\n])'")
REGEX_BEFORE = re.compile(r"(?:^|[(,=:\[!&|?{};+\-*%<>~^]|\b(?:return|typeof|case|in|of|new|delete|void|"
                          r"throw|instanceof|else|do|yield|await))\s*$")


class CodeLexer:
    """Splits one language's lines into code (each string literal as \\0N\\0),
    the literals' values and comment text; strings and block comments carry over."""

    def __init__(self, lang):
        self.lang, self.state, self.tail = lang, None, ""

    def feed(self, line):
        code, strings, comments = [], [], []
        i, n = 0, len(line)
        while True:
            if self.state and self.state[0] == "block":
                depth, j = self.state[1], i
                while j < n and depth:
                    if line.startswith("*/", j):
                        depth, j = depth - 1, j + 2
                    elif self.lang == "rust" and line.startswith("/*", j):
                        depth, j = depth + 1, j + 2
                    else:
                        j += 1
                comments.append(line[i:j])
                self.state, i = (None if depth == 0 else ["block", depth]), j
                if self.state:
                    break
                continue
            if self.state and self.state[0] == "template":
                # Template text and ${...} expressions, which may hold nested templates.
                stack, parts, j = self.state[1], self.state[2], i
                while j < n and stack:
                    c = line[j]
                    if stack[-1] == "text":
                        if c == "\\":
                            j += 1
                        elif c == "`":
                            stack.pop()
                        elif line.startswith("${", j):
                            stack.append(1)
                            j += 1
                    elif c == "`":
                        stack.append("text")
                    elif c in "{}":
                        stack[-1] += 1 if c == "{" else -1
                        if not stack[-1]:
                            stack.pop()
                    j += 1
                parts.append(line[i:min(j, n)])
                if stack:
                    parts.append("\n")
                    break
                strings.append("".join(parts)[:-1])
                code.append(f"\0{len(strings) - 1}\0")
                self.state, i = None, j
                continue
            if self.state:
                _, end, escapes, multiline, parts = self.state
                j = i
                while j < n and not line.startswith(end, j):
                    j += 2 if escapes and line[j] == "\\" else 1
                parts.append(line[i:min(j, n)])
                if j >= n and multiline:
                    parts.append("\n")
                    break
                strings.append("".join(parts))
                code.append(f"\0{len(strings) - 1}\0")
                self.state, i = None, min(j + len(end), n)
                continue
            match = STARTS[self.lang].search(line, i)
            if not match:
                code.append(line[i:])
                break
            code.append(line[i:match.start()])
            token, i = match.group(), match.end()
            if token in ("#", "//"):
                comments.append(line[i:])
                break
            if token == "/*":
                self.state = ["block", 1]
            elif self.lang == "rust" and token == "'":
                char = RUST_CHAR.match(line, match.start())
                if char:
                    strings.append(char.group()[1:-1])
                    code.append(f"\0{len(strings) - 1}\0")
                    i = char.end()
                else:
                    code.append("'")  # a lifetime
            elif self.lang == "node" and token == "/":
                if REGEX_BEFORE.search(self.tail + "".join(code)):
                    j, klass = i, False
                    while j < n and (klass or line[j] != "/"):
                        klass = (klass or line[j] == "[") and line[j] != "]"
                        j += 2 if line[j] == "\\" else 1
                    j += 1
                    while j < n and line[j].isalpha():
                        j += 1
                    strings.append(line[i:j])
                    code.append(f"\0{len(strings) - 1}\0")
                    i = min(j, n)
                else:
                    code.append("/")
            elif self.lang == "python":
                quote = token.lstrip("rRbBuUfF")
                self.state = ["str", quote, True, len(quote) == 3, []]
            elif self.lang == "rust":
                raw = "r" in token
                self.state = ["str", '"' + (match.group(1) or ""), not raw, True, []]
            elif token == "`":
                self.state = ["template", ["text"], []]
            else:
                self.state = ["str", token, True, False, []]
        text = "".join(code)
        self.tail = (self.tail + "\n" + text)[-80:]
        return text, strings, " ".join(comments)


def alias_pattern(lang, text):
    """Calls through import aliases and assigned names of the deletion functions."""
    names, bare, modules = NAMES[lang], set(), set()
    group = "|".join(names)
    if lang == "python":
        modules.update(m[1] for m in re.finditer(r"\bimport\s+(?:os|shutil)\s+as\s+(\w+)", text))
        for m in re.finditer(r"\bfrom\s+(?:os|shutil)\s+import\s+(\([^)]*\)|[^\n;]+)", text):
            for item in m[1].strip("()").split(","):
                words = item.split()
                if words and words[0] in names:
                    bare.add(words[-1])
    elif lang == "rust":
        bare.update(m[2] for m in re.finditer(rf"\b({group})\s+as\s+(\w+)", text))
    else:
        for m in re.finditer(r"\{([^{}]*)\}\s*(?:=|from\b)", text):
            for item in m[1].split(","):
                words = re.split(r"\s+as\s+|\s*:\s*", item.strip())
                if words[0] in names and words[-1]:
                    bare.add(words[-1])
    bare.update(m[1] for m in re.finditer(rf"\b(\w+)\s*=\s*[\w.:$]*\b(?:{group})\s*(?:;|$)", text, re.M))
    patterns = []
    if bare:
        patterns.append(r"(?<![\w$.])(?<!def )(?<!fn )(?<!function )(?:%s)\s*\(" % "|".join(map(re.escape, bare)))
    if modules:
        patterns.append(r"(?<![\w.])(?:%s)\.(?:%s)\s*\(" % ("|".join(modules), "|".join(NAMES["python"])))
    return re.compile("|".join(patterns)) if patterns else None


def runs_deletion(value):
    words = value.split()
    return bool(words) and PROGRAM.fullmatch(words[0]) is not None


def code_hit(lang, code, strings, aliases):
    for match in CALLS[lang].finditer(code):
        if lang == "node" and NODE_DEFINITION[0].fullmatch(code[:match.start()]) \
                and NODE_DEFINITION[1].match(code, match.end()):
            continue  # a method definition named unlink/rm
        return True
    if aliases and aliases.search(code):
        return True
    return any(runs_deletion(strings[int(m[1])]) for m in SPAWN.finditer(code)) or \
        any(PROGRAM.fullmatch(strings[int(m[1])]) for m in ARGV.finditer(code))


# ── Shell: command words ─────────────────────────────────────────────────────
ASSIGNMENT = re.compile(r"[A-Za-z_]\w*(?:\[[^]]*\])?\+?=")
RESERVED = {"if", "then", "else", "elif", "fi", "do", "done", "while", "until", "!", "{", "}", "time",
            "[[", "]]", "case", "esac", "for", "select", "function", "coproc"}
# Wrappers that run their argument as the command; letters are flags taking a value.
WRAPPERS = {"sudo": "ugCDhpRTr", "doas": "uC", "env": "uCSP", "command": "", "builtin": "", "exec": "a",
            "nohup": "", "nice": "n", "xargs": "nILPsdEa", "timeout": "sk", "stdbuf": "ioe", "setsid": "",
            "ionice": "cnp"}
SHELLS = {"sh", "bash", "zsh", "dash", "ksh", "su", "runuser"}
HEREDOC = re.compile(r"\s*(?:'([^']*)'|\"([^\"]*)\"|\\?([^\s;&|<>()]+))")


def is_python(command):
    return re.search(r"python", command, re.I) is not None


def is_node(command):
    return re.search(r"(?:^|/)node(?:js)?$|NODE", command) is not None


def command_word(values):
    """The executed program of a simple command and its index, or (None, _)."""
    i = 0
    while i < len(values):
        value = values[i]
        name = value.rsplit("/", 1)[-1]
        if value in RESERVED or ASSIGNMENT.match(value):
            i += 1
        elif name in WRAPPERS:
            if name == "command" and any(v in ("-v", "-V") for v in values[i + 1:i + 3]):
                return None, i  # a lookup, not a run
            takes, i = WRAPPERS[name], i + 1
            while i < len(values) and (values[i].startswith("-") and values[i] != "-"
                                       or name == "env" and ASSIGNMENT.match(values[i])):
                flag, i = values[i], i + 1
                if len(flag) == 2 and flag[1] in takes:
                    i += 1
            if name == "timeout" and i < len(values):
                i += 1  # the duration
        else:
            return value, i
    return None, i


def option(args, letters):
    for k, arg in enumerate(args[:-1]):
        if arg in letters or re.fullmatch(r"-[A-Za-z]*c[A-Za-z]*", arg) and "-c" in letters:
            return args[k + 1]
    return None


class Sink:
    def __init__(self):
        self.hits, self.records, self.comments, self.code = [], [], {}, set()

    def note(self, number, code, comment):
        if comment:
            self.comments[number] = self.comments.get(number, "") + " " + comment
        if code and code.strip():
            self.code.add(number)


class Shell:
    """Shell lines to simple commands; heredocs for Python or Node go to their lexer."""

    def __init__(self, sink, depth=0, functions=frozenset()):
        self.sink, self.depth, self.functions = sink, depth, functions
        self.heredoc, self.pending, self.frames = None, [], []
        self.words, self.word, self.quote = [], None, None
        self.redirect = self.continued = False
        self.last = 0

    def feed(self, number, line):
        self.last = number
        if self.heredoc:
            delim, strip, lang, sub = self.heredoc
            if (line.lstrip("\t") if strip else line) == delim:
                if lang == "shell":
                    sub.flush()
                self.heredoc = self.pending.pop(0) if self.pending else None
            elif lang == "shell":
                sub.feed(number, line)
            else:
                code, strings, comment = sub.feed(line)
                self.sink.records.append((number, lang, code, strings))
                self.sink.note(number, code, comment)
            return
        i, n = 0, len(line)
        while i < n:
            c = line[i]
            if self.quote == "'":
                j = line.find("'", i)
                self.word[0] += line[i:] if j < 0 else line[i:j]
                self.quote, i = (self.quote, n) if j < 0 else (None, j + 1)
            elif self.quote == '"' and c in '"\\':
                if c == "\\":
                    self.word[0] += line[i + 1:i + 2]
                else:
                    self.quote = None
                i += 1 if c == '"' else 2
            elif line.startswith(("$((", "${"), i):
                j = balanced(line, i + 1)
                self.start(number)
                self.word[0] += line[i:j]
                i = j
            elif line.startswith("$(", i) or self.quote is None and line.startswith(("<(", ">("), i):
                self.open(number, ")", True)
                i += 2
            elif c == "`":
                if self.frames and self.frames[-1][3] == "`":
                    self.close(number)
                else:
                    self.open(number, "`", True)
                i += 1
            elif self.quote == '"':
                self.word[0] += c
                i += 1
            else:
                i = self.unquoted(number, line, i)
        if self.quote:
            self.word[0] += "\n"
        elif self.continued:
            self.continued = False
        else:
            self.end()
            self.finish(number)
            if self.pending and not self.heredoc:
                self.heredoc = self.pending.pop(0)

    def unquoted(self, number, line, i):
        c, n = line[i], len(line)
        if c in " \t":
            self.end()
        elif c == "#" and self.word is None:
            self.sink.note(number, None, line[i + 1:])
            return n
        elif c == "\\":
            if i + 1 == n:
                self.continued = True
            else:
                self.start(number)
                self.word[0] += line[i + 1]
            return i + 2
        elif c in "'\"":
            self.start(number)
            self.quote = c
        elif c == "(" and self.word is None and not self.words:
            if line.startswith("((", i):  # an arithmetic command
                j = balanced(line, i)
                self.start(number)
                self.word[0] += line[i:j]
                return j
            self.open(number, ")", False)  # a subshell
        elif c == ")" and not (self.word and self.word[0].count("(") > self.word[0].count(")")):
            if self.frames and self.frames[-1][3] == ")":
                self.close(number)
            else:
                self.word, self.words = None, []  # a case pattern, not a command
        elif line.startswith("<<", i) and not line.startswith("<<<", i):
            return self.here(line, i)
        elif c in "<>":
            if self.word and self.word[0].isdigit():
                self.word = None  # a file descriptor number
            self.end()
            self.redirect = True
            while i + 1 < n and line[i + 1] in "<>&|":
                i += 1
        elif c in ";&|":
            self.end()
            self.finish(number)
            while i + 1 < n and line[i + 1] in ";&|":
                i += 1
        else:
            self.start(number)
            self.word[0] += c
        return i + 1

    def here(self, line, i):
        strip = line.startswith("<<-", i)
        match = HEREDOC.match(line, i + (3 if strip else 2))
        if not match:
            return i + 2
        self.end()
        command = command_word([value for value, _ in self.words])[0] or ""
        lang = "python" if is_python(command) else "node" if is_node(command) else "shell"
        sub = Shell(self.sink, self.depth, self.functions) if lang == "shell" else CodeLexer(lang)
        self.pending.append((next(g for g in match.groups() if g is not None), strip, lang, sub))
        return match.end()

    def start(self, number):
        if self.word is None:
            self.word = ["", number]

    def end(self):
        if self.word is not None:
            if not self.redirect:
                self.words.append(tuple(self.word))
            self.word, self.redirect = None, False

    def open(self, number, closer, substitution):
        if substitution:
            self.start(number)
        self.frames.append((self.words, self.word, self.quote, closer, substitution))
        self.words, self.word, self.quote = [], None, None

    def close(self, number):
        self.end()
        self.finish(number)
        self.words, self.word, self.quote, _, substitution = self.frames.pop()
        if substitution:
            self.word[0] += "$(...)"

    def finish(self, number):
        if self.words:
            for _, line in self.words:
                self.sink.code.add(line)
            if self.deletes([value for value, _ in self.words]):
                self.sink.hits.append((self.words[0][1], number))
        self.words = []

    def flush(self):
        self.end()
        self.finish(self.last)

    def deletes(self, values):
        command, i = command_word(values)
        if command is None:
            return False
        args, name = values[i + 1:], command.rsplit("/", 1)[-1]
        if PROGRAM.fullmatch(command):
            return True
        if command in self.functions and args and PROGRAM.fullmatch(args[0]):
            return True  # a wrapper this file defines, such as as_root rm
        if name == "find":
            return "-delete" in args or any(arg in ("-exec", "-execdir", "-ok", "-okdir") and k + 1 < len(args)
                                            and PROGRAM.fullmatch(args[k + 1]) for k, arg in enumerate(args))
        if self.depth >= 4:
            return False
        if name == "trap" and args:
            return snippet_deletes("shell", args[0], self.depth + 1, self.functions)
        if name == "eval":
            return snippet_deletes("shell", " ".join(args), self.depth + 1, self.functions)
        language = "shell" if name in SHELLS else "python" if is_python(command) else \
            "node" if is_node(command) else None
        script = option(args, ("-e", "--eval", "-p", "--print") if language == "node" else ("-c",))
        return language is not None and script is not None and \
            snippet_deletes(language, script, self.depth + 1, self.functions)


def balanced(line, i):
    """The index after the bracket closing the one at line[i] (or the line's end)."""
    opener = line[i]
    closer, depth = {"(": ")", "{": "}"}[opener], 0
    for j in range(i, len(line)):
        depth += (line[j] == opener) - (line[j] == closer)
        if depth == 0:
            return j + 1
    return len(line)


def scan(lang, lines, depth=0, functions=None):
    """Deletion spans (first line, last line), comments and code lines of one source."""
    sink = Sink()
    if lang == "shell":
        if functions is None:
            text = "\n".join(lines)
            functions = frozenset(m[1] or m[2] for m in re.finditer(
                r"^\s*(?:function\s+([A-Za-z_][\w-]*)|([A-Za-z_][\w-]*)\s*\(\))", text, re.M))
        shell = Shell(sink, depth, functions)
        for number, line in enumerate(lines, 1):
            shell.feed(number, line)
        shell.flush()
    else:
        lexer = CodeLexer(lang)
        for number, line in enumerate(lines, 1):
            code, strings, comment = lexer.feed(line)
            sink.records.append((number, lang, code, strings))
            sink.note(number, code, comment)
    for language in {record[1] for record in sink.records}:
        records = [record for record in sink.records if record[1] == language]
        aliases = alias_pattern(language, "\n".join(record[2] for record in records))
        sink.hits.extend((number, number) for number, _, code, strings in records
                         if code_hit(language, code, strings, aliases))
    return sink


def snippet_deletes(lang, text, depth, functions):
    return bool(scan(lang, text.split("\n"), depth, functions).hits)


# ── Files and revisions ──────────────────────────────────────────────────────
def git(*args, check=True):
    result = subprocess.run(["git", "-c", "core.quotePath=false", *args], encoding="utf-8",
                            errors="replace", stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if check and result.returncode:
        raise SystemExit(f"removal-guard: git {' '.join(args)} failed: {result.stderr.strip()}")
    return result


def unquote(path):
    if path.startswith('"'):
        raw = codecs.escape_decode(path[1:-1].encode())[0]
        return raw.decode("utf-8", "surrogateescape")
    return path


def language(path, first_line):
    """The product language of `path`, or None for tests, fixtures and non-code."""
    parts = PurePosixPath(path).parts
    if path in EXEMPT_FILES or any(part in EXEMPT_DIRS for part in parts[:-1]):
        return None
    name = parts[-1]
    if TEST_NAME.search(name):
        return None
    suffix = PurePosixPath(name).suffix
    if suffix:
        return LANGUAGES.get(suffix)
    if first_line.startswith("#!"):
        for word, found in (("python", "python"), ("node", "node"), ("bash", "shell"), ("sh", "shell")):
            if word in first_line:
                return found
    return None


def rust_test_lines(lines):
    """1-based line numbers inside #[cfg(test)] / #[test] items of rustfmt'd source."""
    inside = set()
    index = 0
    while index < len(lines):
        match = RUST_TEST_ATTRIBUTE.match(lines[index])
        if not match:
            index += 1
            continue
        indent, start = match[1], index
        item = index + 1
        while item < len(lines) and lines[item].strip().startswith(("#[", "///", "//")):
            item += 1
        end = item
        if item < len(lines) and not lines[item].rstrip().endswith(";") and \
                not re.search(r"\{.*\}\s*$", lines[item]):
            end = item + 1
            while end < len(lines) and not lines[end].startswith(indent + "}"):
                end += 1
        inside.update(range(start + 1, min(end, len(lines) - 1) + 2))
        index = end + 1
    return inside


def rust_test_module(path, read, depth=0):
    """True when `path` is a module file declared, directly or through its
    parent modules, inside #[cfg(test)]."""
    file = PurePosixPath(path)
    if file.name in ("lib.rs", "main.rs") or depth > 16:
        return False
    stem = file.parent.name if file.name == "mod.rs" else file.stem
    directory = file.parent.parent if file.name == "mod.rs" else file.parent
    parents = [directory / "mod.rs", directory / "lib.rs", directory / "main.rs",
               directory.parent / (directory.name + ".rs")]
    declaration = re.compile(rf"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+{re.escape(stem)}\s*;")
    for parent in parents:
        text = read(str(parent))
        if text is None:
            continue
        lines = text.split("\n")
        for number, line in enumerate(lines, 1):
            if declaration.match(line):
                return number in rust_test_lines(lines) or rust_test_module(str(parent), read, depth + 1)
    return False


def marked(sink, start, end):
    if any(MARKER.search(sink.comments.get(number, "")) for number in range(start, end + 1)):
        return True
    above = start - 1
    return above not in sink.code and bool(MARKER.search(sink.comments.get(above, "")))


class Reader:
    """File text at one revision, read once through one `git cat-file --batch`."""

    def __init__(self, revision):
        self.revision, self.cache = git("rev-parse", "--verify", f"{revision}^{{commit}}").stdout.strip(), {}
        self.batch = subprocess.Popen(["git", "cat-file", "--batch"], stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE)

    def __call__(self, path):
        if path not in self.cache:
            self.batch.stdin.write(f"{self.revision}:{path}\n".encode())
            self.batch.stdin.flush()
            header = self.batch.stdout.readline().decode().split()
            text = None
            if len(header) == 3:
                data = self.batch.stdout.read(int(header[2]) + 1)[:-1]
                if header[1] == "blob":
                    text = data.decode("utf-8", "replace")
            self.cache[path] = text
        return self.cache[path]

    def close(self):
        self.batch.stdin.close()
        self.batch.wait()


def findings(path, numbers, read):
    """(line, lang, text, marked) for each raw deletion touching `numbers` of product file `path`."""
    text = read(path)
    if text is None:
        return []
    lines = text.split("\n")
    lang = language(path, lines[0] if lines else "")
    if lang is None or (lang == "rust" and rust_test_module(path, read)):
        return []
    tests = rust_test_lines(lines) if lang == "rust" else set()
    sink = scan(lang, lines)
    wanted, found = set(numbers), {}
    for start, end in sorted(set(sink.hits)):
        touched = [number for number in range(start, end + 1) if number in wanted]
        if touched and start not in tests and touched[0] not in found:
            found[touched[0]] = (touched[0], lang, lines[start - 1].strip(), marked(sink, start, end))
    return [found[number] for number in sorted(found)]


def added_lines(base, head):
    diff = git("diff", "-U0", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/",
               f"{base}...{head}").stdout
    added, path, number, header = {}, None, 0, False
    for line in diff.split("\n"):
        if line.startswith("diff --git "):
            path, header = None, True
        elif header and line.startswith("+++ "):
            target = line[4:]
            path = None if target == "/dev/null" else unquote(target)[2:]
        elif line.startswith("@@"):
            header = False
            number = int(re.match(r"@@ -\S+ \+(\d+)", line)[1])
        elif path and not header and line.startswith("+"):
            added.setdefault(path, set()).add(number)
            number += 1
    return added


def check(base, head):
    if git("rev-parse", "--verify", "--quiet", f"{base}^{{commit}}", check=False).returncode:
        print(f"removal-guard: base {base} is unavailable; fetch it "
              "(git fetch origin +refs/heads/develop:refs/remotes/origin/develop)", file=sys.stderr)
        return 2
    read = Reader(head)
    refused = []
    for path, numbers in sorted(added_lines(base, head).items()):
        for number, lang, text, ok in findings(path, numbers, read):
            if not ok:
                refused.append((path, number, lang, text))
    read.close()
    for path, number, lang, text in refused:
        comment = "//" if lang in ("rust", "node") else "#"
        print(f"{path}:{number}: raw deletion added to product code: {text}\n"
              f"  fix: call {HELPER[lang]}, or end the line with "
              f"'{comment} removal-ok: <why this exact path cannot reach outside what this code created>'")
    if refused:
        print(f"removal-guard: {len(refused)} raw deletion(s) refused "
              "(exact owned paths only, every parent checked without following links)", file=sys.stderr)
        return 1
    print(f"removal-guard: no unguarded deletion added in {base}...{head}")
    return 0


def inventory(head):
    read = Reader(head)
    files = git("ls-tree", "-r", "--name-only", "-z", head).stdout.split("\0")
    counts = {}
    for path in filter(None, files):
        if PurePosixPath(path).suffix not in LANGUAGES and "." in PurePosixPath(path).name:
            continue
        text = read(path)
        if not text:
            continue
        for number, lang, line, ok in findings(path, range(1, text.count("\n") + 2), read):
            fd_relative = bool(re.search(r"unlinkat|dir_fd\s*=", line))
            state = "marked" if ok else "fd-relative" if fd_relative else "raw"
            total = counts.setdefault(lang, {"raw": 0, "fd-relative": 0, "marked": 0})
            total[state] += 1
            print(f"{path}:{number}: {lang}: {state}: {line}")
    read.close()
    for lang, total in sorted(counts.items()):
        print(f"removal-guard inventory: {lang}: {sum(total.values())} call(s): "
              + ", ".join(f"{count} {state}" for state, count in total.items()))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--base", default="origin/develop")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--inventory", action="store_true")
    args = parser.parse_args()
    return inventory(args.head) if args.inventory else check(args.base, args.head)


if __name__ == "__main__":
    sys.exit(main())
