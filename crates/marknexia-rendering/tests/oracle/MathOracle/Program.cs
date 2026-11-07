// Writes crates/marknexia-rendering/tests/oracle/math.v1.json: the .NET
// MarkdownRenderer output (Markdig math spans, SimpleMathRenderer, the
// HtmlSanitizer pass and the template) for each math case below.
//
// Usage: dotnet run --project <this folder> -- <output.json> <source-revision> <src-tree>
// The case list is mirrored in tests/math_oracle.rs only through the JSON file.

using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using Marknexia.Core;
using Marknexia.Rendering;

if (args.Length != 3) throw new ArgumentException("usage: <output.json> <source-revision> <src-tree>");

string Repeat(string text, int count) => string.Concat(Enumerable.Repeat(text, count));

var cases = new (string Name, string Markdown, bool EnableMath)[]
{
    ("inline-basic", "Inline $x^2 + y_1$ and $a_{ij}$.\n", true),
    ("display-block", "$$\nE = mc^2\n$$\n", true),
    ("display-inline", "Text $$a+b$$ text\n", true),
    ("greek-lower", @"$\alpha \beta \gamma \delta \epsilon \varepsilon \zeta \eta \theta \vartheta \iota \kappa \lambda \mu \nu \xi \pi \varpi \rho \sigma \tau \upsilon \phi \varphi \chi \psi \omega$" + "\n", true),
    ("greek-upper", @"$\Gamma \Delta \Theta \Lambda \Xi \Pi \Sigma \Phi \Psi \Omega$" + "\n", true),
    ("operators", @"$\infty \pm \mp \times \cdot \div \le \leq \ge \geq \neq \ne \approx \equiv$" + "\n", true),
    ("arrows-and-sets", @"$\to \rightarrow \leftarrow \leftrightarrow \in \notin \subset \subseteq \cup \cap$" + "\n", true),
    ("big-operators-and-logic", @"$\sum \prod \int \partial \nabla \forall \exists \land \lor \neg \ldots \dots \ell$" + "\n", true),
    ("fractions", @"$\frac{a}{b} + \dfrac{1}{2} - \tfrac{x}{y}$" + "\n", true),
    ("fraction-atoms", @"$\frac12 \frac a b$" + "\n", true),
    ("square-roots", @"$\sqrt{x} + \sqrt[3]{y} + \sqrt 2$" + "\n", true),
    ("nested", @"$\frac{\sqrt{a^2+b^2}}{c_{n}}$" + "\n", true),
    ("text-and-operatorname", @"$\text{if } x \operatorname{sin} y$" + "\n", true),
    ("left-right", @"$\left( x \right) \left. y \right| \left[ z \right]$" + "\n", true),
    ("left-at-end", @"$a \left$" + "\n", true),
    ("single-character-commands", @"$\{ a \} \, b \; c \! d \_ f \% g$" + "\n", true),
    ("unknown-commands", @"$\foo + \Alpha + \mathbb{R}$" + "\n", true),
    ("braced-superscript-quirk", @"$x^{2} y_{ab}$" + "\n", true),
    ("command-scripts", @"$x^\alpha y_\beta z^\frac{1}{2}$" + "\n", true),
    ("white-space-arguments", @"$x^ 2 y_  3$" + "\n", true),
    ("script-at-end", @"$x^$ and $y_$" + "\n", true),
    ("html-special-characters", "$a<b$ and $c>d$ and $a&b$ and $\"q\"$ and $'s'$\n", true),
    ("latin-1", "$é^ü \\pm ×$\n", true),
    ("astral", "$𝑥^2 + \\𝑥$\n", true),
    ("unbalanced", @"$\frac{a$ and $\sqrt[3$ and ${{{x$ and $}}}y$" + "\n", true),
    ("empty-groups", @"$\frac{}{} x^{} \sqrt{}$" + "\n", true),
    ("leading-space", "$ x^2 $ and $x^2 $\n", true),
    ("math-disabled", "Inline $x^2$ and\n\n$$\ny < z\n$$\n", false),
    ("code-is-not-math", "`$x^2$` and\n\n```\n$y^2$\n```\n", true),
    ("escaped-dollars", "Price \\$5 and \\$6\n", true),
    ("raw-html-math", "<span class=\"math\">\\(x^2\\)</span>\n\n<div class=\"math\">$$<img src=x onerror=alert(1)>$$</div>\n", true),
    ("raw-html-math-escaped-in-paragraph", "<span class=\"math\">\\[a_1\\]</span>\n", true),
    ("raw-html-math-block-inline", "<div class=\"math\">\\(x^2 < y\\)</div>\n", true),
    ("raw-html-math-upper-case", "<DIV CLASS=\"math\">\\[a_1\\]</DIV>\n", true),
    ("raw-html-math-unterminated", "<span class=\"math\">\\(x^2</span> <span class=\"math\">$$y</span>\n", true),
    ("moderate-nesting", "$" + Repeat(@"\frac{", 8) + "x" + Repeat("}{y}", 8) + "$\n", true),
    ("deep-nesting", "$" + Repeat(@"\sqrt{", 20) + "x" + Repeat("}", 20) + "$\n", true),
    ("caret-backslash-at-end", "<div class=\"math\">\\(x^\\\\)</div>\n", true),
    ("underscore-backslash-at-end", "<div class=\"math\">$$y_\\$$</div>\n", true),
    ("multi-line-display", "$$\n\\frac{a}{b}\n\\\\\nc\n$$\n", true),
    ("list-items", "- item $x$\n- $$y$$\n", true),
    ("heading", "# Title $x^2$\n", true),
    ("table", "| a | b |\n|---|---|\n| $x$ | $y_1$ |\n", true),
};

var renderer = new MarkdownRenderer();
string root = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", "..", "..", "..", ".."));
if (!File.Exists(Path.Combine(root, "Cargo.toml"))) throw new InvalidOperationException($"repository root not found from {AppContext.BaseDirectory}");
var output = new JsonArray();
bool firstPage = true;
foreach (var (name, markdown, enableMath) in cases)
{
    var entry = new JsonObject { ["name"] = name, ["markdown"] = markdown, ["enableMath"] = enableMath };
    try
    {
        RenderedDocument document = await renderer.RenderAsync(markdown,
            new RenderContext(Path.Combine(root, "compat", "virtual.md"), root, AppTheme.System, EnableMath: enableMath));
        string page = Normalize(document.HtmlContent);
        const string open = "<div class=\"markdown-body\">\n";
        const string close = "\n  </div>\n  <script";
        int start = page.IndexOf(open, StringComparison.Ordinal) + open.Length;
        int end = page.LastIndexOf(close, StringComparison.Ordinal);
        entry["outcome"] = "rendered";
        entry["body"] = page[start..end];
        if (firstPage)
        {
            entry["page"] = page;
            firstPage = false;
        }
    }
    catch (Exception exception) when (exception is not OutOfMemoryException)
    {
        entry["outcome"] = "exception";
        entry["exception"] = exception.GetType().Name;
    }
    output.Add(entry);
}

var result = new JsonObject
{
    ["schemaVersion"] = "marknexia-math-oracle-v1",
    ["generator"] = "crates/marknexia-rendering/tests/oracle/MathOracle",
    ["sourceRevision"] = args[1],
    ["srcTree"] = args[2],
    ["context"] = "MarkdownRenderer defaults; RenderContext(compat/virtual.md, repository root, System theme, remote assets off); page newline-normalized; document host and nonce replaced by {volatile}",
    ["cases"] = output,
};
var options = new JsonSerializerOptions { WriteIndented = true, Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping };
File.WriteAllText(args[0], result.ToJsonString(options).Replace("\r\n", "\n") + "\n");
Console.WriteLine($"wrote {output.Count} cases to {args[0]}");

// The ParityNormalizer rules (tools/Marknexia.ParityExporter), restated so
// the oracle needs no change to .NET sources or the exporter.
static string Normalize(string html)
{
    string normalized = Regex.Replace(html, "\\r\\n?|\\n", "\n");
    Match document = Regex.Match(normalized,
        "<!DOCTYPE html>[\\s\\S]*?<base href=\"https://(?<host>document-[0-9a-f]{32}\\.marknexia\\.viewer)/compat/\"[\\s\\S]*?<script nonce=\"(?<nonce>[A-Za-z0-9+/=]+)\">\\nwindow\\.marknexiaBridge =",
        RegexOptions.IgnoreCase);
    if (!document.Success) throw new InvalidOperationException("unexpected template");
    string host = document.Groups["host"].Value;
    string nonce = document.Groups["nonce"].Value;
    normalized = Regex.Replace(normalized, $"(?<=<base href=\"https://){Regex.Escape(host)}(?=/compat/\")", "document-{volatile}.marknexia.viewer");
    normalized = Regex.Replace(normalized, $"(?<=(?:base-uri|img-src) https://){Regex.Escape(host)}(?=[ ;])", "document-{volatile}.marknexia.viewer");
    normalized = Regex.Replace(normalized, $"(?<=script-src &#39;nonce-){Regex.Escape(nonce)}(?=&#39;)", "{volatile}");
    return Regex.Replace(normalized, $"<script nonce=\"{Regex.Escape(nonce)}\">\\nwindow\\.marknexiaBridge =", "<script nonce=\"{volatile}\">\nwindow.marknexiaBridge =");
}
