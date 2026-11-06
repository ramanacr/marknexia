# Highlighting parity (HL-1 to HL-4)

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-rendering/src/highlight/` (the `ColorCodeSyntaxHighlighter` port). Owner decision G1 (2026-11-06): highlighting must cover every language that ColorCode supports, with parity to the .NET output. This record replaces the C#-only part of REND-2 in `rendering-differences.md`.

## What the port covers

`ColorCodeSyntaxHighlighter.HighlightCode` (`src/Marknexia.Syntax`) with ColorCode.Core and ColorCode.HTML 2.0.15:

* all 25 ColorCode languages: `javascript`, `html`, `c#`, `vb.net`, `ashx`, `asax`, `aspx`, `aspx(c#)`, `aspx(vb.net)`, `sql`, `xml`, `php`, `css`, `cpp`, `java`, `json`, `powershell`, `typescript`, `f#`, `koka`, `haskell`, `markdown`, `fortran`, `python`, `matlab`;
* Marknexia's `ResolveLanguage` (`c#`/`cs`, `c++`/`c`, `js`, `ts`, `md`, `ps1`/`posh`) and ColorCode's `FindById` (id or alias, first match in repository order);
* `LanguageParser`: the compiled alternation of every rule, every capture of every group (`Group.Captures`), nested languages (`&javascript` inside `<script>`, `&c#` and `&vb.net` in ASP.NET pages), and the scope-tree and nested-offset quirks of ColorCode 2.0.15;
* `HtmlClassFormatter`: `<div class="{CssClassName}"><pre>`, spans with the `StyleDictionary.DefaultLight` class (a bare `<span>` for a scope without a style, such as Python's `Intrinsic`), stable insertion order, and `WebUtility.HtmlEncode` text (`&#39;`, `&#233;` for U+00A0 to U+00FF, `&#128512;` for astral characters, U+FFFD for a surrogate pair that a span boundary splits);
* the `catch` fallback: where .NET throws (an out-of-range `Substring`, a missing nested language, a scope with two parents), Rust returns the same plain `<pre><code>`.

The language data is not hand-copied. `crates/marknexia-rendering/tests/oracle/HighlightOracle` (a .NET 10 console project that references `src/Marknexia.Syntax` and changes no .NET source) generates:

* `src/highlight/languages.rs`: each language's id, CSS class, aliases (every string literal of ColorCode.Core probed through `HasAlias`), the exact regex string that `LanguageCompiler` builds, its capture-to-scope list and group count, and the style map;
* `src/highlight/unicode.rs`: the .NET `\w`, `\d`, `\s` and `\b` word sets and the `RegexOptions.IgnoreCase` equivalences, measured on all 65,536 UTF-16 code units;
* `tests/oracle/highlight.v1.json`: the .NET output for 522 cases.

### Regex engine

The ColorCode rules need .NET regex features that the `regex` crate (MIT/Apache-2.0) does not have: variable-length lookbehind (`(?<=<%@.+?language="c\#".*?%>.*?<script.*?runat="server">)`), atomic groups, balancing groups (`(?<-comment>-})`), conditionals (`(?(type)[,])`) and capture history. `fancy-regex` (MIT) has lookbehind and atomic groups but no balancing groups, conditionals or capture history, and would still add a dependency. The port therefore uses a Marknexia-owned backtracking engine (`src/highlight/regex.rs`, no new crate, licences unchanged). It reproduces:

* the .NET syntax the patterns use: inline options `(?imsx-imsx)` and `(?imsx-imsx:…)` with group scope, `IgnorePatternWhitespace` blanks and comments, `{` as a literal when it is not a quantifier, class ranges after a range (`[a-z0-9-_]`), and .NET group numbering (unnamed groups first, then named groups);
* matching on UTF-16 code units with the measured tables;
* leftmost, priority-ordered backtracking with the `RegexInterpreter` loop rules: an iteration that matches empty ends a loop once the minimum count is reached, and its captures stay;
* atomic lookarounds, lookbehind matched right to left, captures undone on backtracking, and balancing pops.

Unsupported syntax (back-references, `\p{…}`, class subtraction, POSIX classes, numbered names) is rejected when the pattern is compiled. No ColorCode pattern uses it, and `every_language_compiles_with_the_dotnet_group_count` checks that each pattern compiles with the .NET group count.

C# keeps the linear hand-written tokenizer of the earlier C#-only highlighter (`src/highlight/csharp.rs`). The .NET C# regexes are quadratic on unterminated quotes, `/*`, `[type:"` and blank runs, so a large C# block would otherwise reach the work budget (HL-1). `csharp_engine_path_matches_the_oracle` checks that the engine gives the .NET output for every C# case as well. The fuzz target and `tests/highlight_invariants.rs` compare the two paths on random input.

## Oracle evidence

`tests/highlight_oracle.rs` highlights every case of `tests/oracle/highlight.v1.json`:

* **516 of 522 cases match byte for byte.** For each of the 25 languages, the cases are 9 generic inputs (empty input, plain words, mixed punctuation and quotes, unterminated constructs, CRLF lines, numbers, Unicode with `ſ`, U+212A KELVIN SIGN, `İ`, `ı`, `ß`, `Σσς`, emoji and a joiner, white space only, no final newline), 6 seeded token soups over a pool of every language's syntax, and 2 long lines. There are also 36 hand-written samples (including `<script>` blocks, nested Koka and Haskell comments, ASP.NET pages with C# and VB server code, and Python docstrings after `:` and `=`) and 61 language-resolution inputs (aliases, case, white space, unknown and empty names).
* **6 cases differ**, all HL-1. Each is listed in `KNOWN_DIFFERENCES`, and its Rust output is pinned in `tests/oracle/highlight.known-differences.v1.json` (the plain fallback). The test fails if a listed case starts to match or if its output changes:

| Case | .NET | Rust |
| --- | --- | --- |
| `json/soup-2`, `json/long-unterminated-quotes`, `koka/soup-2`, `koka/soup-5`, `koka/long-unterminated-quotes` | more than 20 s for one regex match; the oracle stopped it with `REGEX_DEFAULT_MATCH_TIMEOUT` and recorded `timeout` | budget exhausted, plain fallback |
| `json/soup-3` (300 characters) | highlighted after about 3 s | budget exhausted after 134 million steps (about 1 s; it needs 402 million), plain fallback |

The oracle run used .NET 10.0.14 with the current culture `en-IN`.

### Regenerating the oracle

Run this from the repository root with the .NET 10 SDK. The `src/` tree must still be the one recorded (`git rev-parse HEAD:src` prints `c91f107edd5baa3bcba27723f19330cb35837273`, the tree of the fixture revision `38b39f6`):

```powershell
dotnet run --project crates/marknexia-rendering/tests/oracle/HighlightOracle -- . 38b39f668ce52026261ac4802602c0cf7e0711a3
```

It rewrites `languages.rs`, `unicode.rs` and `highlight.v1.json`. The five timeout cases take 20 s or more each, and the run takes about 10 minutes. Then delete `HighlightOracle/bin` and `HighlightOracle/obj`, and review the diff as a baseline change. If `src/` or the ColorCode package version has changed, regenerate only with owner approval.

## HL-1 — A regex work budget replaces unbounded backtracking

.NET runs ColorCode's regexes without a timeout. Several rules backtrack exponentially on some inputs (the JSON and Koka rules on the token soups above) or quadratically on unterminated constructs, so .NET can highlight one small block for minutes or hours.

Rust counts every engine instruction, every backtrack and every character a loop scans. One budget serves a whole document: it starts at 2^27 steps (134,217,728; about 1 s of matching on the development machine, where the engine runs at about 130 million steps per second) and gains 512 steps for each UTF-16 code unit of each highlighted block. When a block exhausts the budget, that block falls back to the plain `<pre><code>` output, as if .NET had thrown. Later blocks then have only their own per-unit credit.

| Input | .NET | Rust |
| --- | --- | --- |
| A block that needs more steps than the budget left | highlighted, after as long as backtracking takes | plain `<pre><code>` fallback |
| Many such blocks in one document | each one backtracks fully | the first one spends the shared base; the rest get only their per-unit credit (300 copies of `json/soup-3`: 1.8 s in total, all plain) |

The budget is sized from the oracle. Ordinary long lines need at most 140 steps per unit (`sql/long-line`), which is under half of the 512 credit. The largest ordinary short case needs 60,590 steps (`koka/strings`), under 1/16 of the base. Both are checked by `ordinary_code_uses_a_small_part_of_the_budget`. Ordinary blocks of 1.3 MB in every language highlight without reaching the budget (`large_ordinary_blocks_highlight_in_every_language`). The slowest languages are Fortran (6.3 s), Haskell (4.6 s) and MATLAB, Koka and Python (2.5 to 2.6 s), each including the sanitizer and a page of up to 15 MB. A hostile document is bounded by about 1 s plus about 4 µs per code unit of matching. A highlighting pass cannot be cancelled in the middle of a block; `render_cancellable` stops it between stages only.

## HL-2 — Highlighted text is encoded exactly like `WebUtility.HtmlEncode`

The earlier highlighter used the renderer's DOM-equivalent encoder (only `<`, `>`, `&`, `"` and `'`). Highlighted and plain code blocks now use the exact .NET encoding, so `é` becomes `&#233;` and `😀` becomes `&#128512;` before sanitization, as in .NET. After sanitization the page is the same as before, because the sanitizer reserializes text. The pass budget (REND-1) counts the encoded size: a block of Latin-1 text is up to three times larger before sanitization, as it is in .NET.

## HL-3 — Character tables are fixed to the oracle culture

.NET builds `IgnoreCase` sets when a regex is constructed, from the current culture (the Turkish cultures treat `i`, `İ`, `ı` and `I` differently). The Rust tables were measured with the culture `en-IN` and are the same for every user. Under a Turkish culture, .NET matches `(?i)` keywords that contain `i` differently, and Rust keeps the non-Turkish behaviour. The `\w`, `\d`, `\s` and `\b` sets do not depend on the culture.

## HL-4 — Wrapper newlines and the C# fast path

* `HtmlClassFormatter` writes `Environment.NewLine` after `<pre>` and before `</pre>` (`\r\n` on Windows). Rust writes `\n`, as for the whole page (REND-3). The oracle normalizes only these two newlines; newlines inside the code are compared as they are.
* C# runs through the linear tokenizer, not the engine, so C# never reaches the budget. Its output equals the engine's and .NET's on the whole oracle corpus. A C# input on which the engine's budget would run out is highlighted by the tokenizer, as .NET would eventually do.

## Binary size

`marknexia-win32.exe` built in release for x64 (`strip = "symbols"`, thin LTO): 7,230,976 bytes at `42f347b`, and 7,451,136 bytes with this change. The change adds 220,160 bytes (215 KiB): the engine, the language table (66 KB of source) and the Unicode tables. The unpacked payload stays far below the 35 MB budget.

## Fuzzing

`fuzz/fuzz_targets/highlight.rs` takes a language selector byte and UTF-8 code. It checks that nothing panics, that the output is the language wrapper or the plain fallback, that removing the spans and decoding the entities gives back the input (for input without astral characters), and that the C# fast path equals the engine when the engine finishes. It uses a budget of 2^22 steps, so catastrophic inputs stay fast. The seed corpus `fuzz/corpus/highlight` holds 39 files from the oracle samples. **The libFuzzer target was type-checked only and was not run on this machine** (see the Fuzzing section of `docs/superpowers/specs/2026-09-12-rust-content-dependency-decision.md`). `crates/marknexia-rendering/tests/highlight_invariants.rs` checks the same invariants on 2,000 seeded token soups in every language on each `cargo test`.

## Approval needed

Task 8 must approve or change:

1. The work budget and its plain fallback (HL-1), including the base of 2^27 steps per document and 512 steps per code unit.
2. The culture-fixed character tables (HL-3).

HL-2 and HL-4 are recorded so reviewers can see them.
