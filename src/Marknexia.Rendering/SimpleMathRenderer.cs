using System.Net;
using System.Text;
using Marknexia.Core;

namespace Marknexia.Rendering;

/// <summary>
/// Renders the small, predictable subset of TeX commonly used in README files
/// without loading a remote font, script, or stylesheet. Unrecognised commands
/// remain visible as encoded text rather than being interpreted as HTML.
/// </summary>
public sealed class SimpleMathRenderer : IMathRenderer
{
    private static readonly IReadOnlyDictionary<string, string> Symbols =
        new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["alpha"] = "α", ["beta"] = "β", ["gamma"] = "γ", ["delta"] = "δ",
            ["epsilon"] = "ϵ", ["varepsilon"] = "ε", ["zeta"] = "ζ", ["eta"] = "η",
            ["theta"] = "θ", ["vartheta"] = "ϑ", ["iota"] = "ι", ["kappa"] = "κ",
            ["lambda"] = "λ", ["mu"] = "μ", ["nu"] = "ν", ["xi"] = "ξ",
            ["pi"] = "π", ["varpi"] = "ϖ", ["rho"] = "ρ", ["sigma"] = "σ",
            ["tau"] = "τ", ["upsilon"] = "υ", ["phi"] = "ϕ", ["varphi"] = "φ",
            ["chi"] = "χ", ["psi"] = "ψ", ["omega"] = "ω",
            ["Gamma"] = "Γ", ["Delta"] = "Δ", ["Theta"] = "Θ", ["Lambda"] = "Λ",
            ["Xi"] = "Ξ", ["Pi"] = "Π", ["Sigma"] = "Σ", ["Phi"] = "Φ",
            ["Psi"] = "Ψ", ["Omega"] = "Ω",
            ["infty"] = "∞", ["pm"] = "±", ["mp"] = "∓", ["times"] = "×",
            ["cdot"] = "⋅", ["div"] = "÷", ["le"] = "≤", ["leq"] = "≤",
            ["ge"] = "≥", ["geq"] = "≥", ["neq"] = "≠", ["ne"] = "≠",
            ["approx"] = "≈", ["equiv"] = "≡", ["to"] = "→", ["rightarrow"] = "→",
            ["leftarrow"] = "←", ["leftrightarrow"] = "↔", ["in"] = "∈",
            ["notin"] = "∉", ["subset"] = "⊂", ["subseteq"] = "⊆", ["cup"] = "∪",
            ["cap"] = "∩", ["sum"] = "∑", ["prod"] = "∏", ["int"] = "∫",
            ["partial"] = "∂", ["nabla"] = "∇", ["forall"] = "∀", ["exists"] = "∃",
            ["land"] = "∧", ["lor"] = "∨", ["neg"] = "¬", ["ldots"] = "…",
            ["dots"] = "…", ["ell"] = "…"
        };

    public MathRenderResult Render(string expression, MathMode mode)
    {
        string normalized = StripDelimiters(expression ?? string.Empty).Trim();
        string html = RenderExpression(normalized);
        string accessibleText = normalized.Length == 0 ? "Mathematical expression" : normalized;
        string encodedLabel = WebUtility.HtmlEncode(accessibleText);
        string className = mode == MathMode.Display ? "marknexia-math-display" : "marknexia-math";
        string tag = mode == MathMode.Display ? "div" : "span";

        return new MathRenderResult(
            $"<{tag} class=\"{className}\" role=\"math\" aria-label=\"{encodedLabel}\">{html}</{tag}>",
            accessibleText);
    }

    private static string StripDelimiters(string expression)
    {
        if (expression.StartsWith(@"\(", StringComparison.Ordinal) && expression.EndsWith(@"\)", StringComparison.Ordinal))
        {
            return expression[2..^2];
        }

        if (expression.StartsWith(@"\[", StringComparison.Ordinal) && expression.EndsWith(@"\]", StringComparison.Ordinal))
        {
            return expression[2..^2];
        }

        if (expression.StartsWith("$$", StringComparison.Ordinal) && expression.EndsWith("$$", StringComparison.Ordinal))
        {
            return expression[2..^2];
        }

        return expression;
    }

    private static string RenderExpression(string expression)
    {
        var output = new StringBuilder(expression.Length + 32);
        for (int index = 0; index < expression.Length; index++)
        {
            char current = expression[index];
            if (current is '^' or '_')
            {
                string argument = ReadAtom(expression, ref index);
                string rendered = RenderExpression(argument);
                string tag = current == '^' ? "sup" : "sub";
                output.Append('<').Append(tag).Append('>').Append(rendered).Append("</").Append(tag).Append('>');
                continue;
            }

            if (current == '\\')
            {
                string command = ReadCommand(expression, ref index);
                if (command is "frac" or "dfrac" or "tfrac")
                {
                    string numerator = ReadGroup(expression, ref index);
                    string denominator = ReadGroup(expression, ref index);
                    output.Append("<span class=\"marknexia-fraction\"><span class=\"marknexia-numerator\">")
                        .Append(RenderExpression(numerator))
                        .Append("</span><span class=\"marknexia-denominator\">")
                        .Append(RenderExpression(denominator))
                        .Append("</span></span>");
                    continue;
                }

                if (command is "sqrt")
                {
                    SkipOptionalGroup(expression, ref index);
                    string radicand = ReadGroup(expression, ref index);
                    output.Append("<span class=\"marknexia-sqrt\"><span aria-hidden=\"true\">√</span><span class=\"marknexia-radicand\">")
                        .Append(RenderExpression(radicand))
                        .Append("</span></span>");
                    continue;
                }

                if (command is "text" or "operatorname")
                {
                    string text = ReadGroup(expression, ref index);
                    output.Append(RenderExpression(text));
                    continue;
                }

                if (command is "left" or "right")
                {
                    if (index + 1 < expression.Length)
                    {
                        char delimiter = expression[++index];
                        if (delimiter != '.') output.Append(WebUtility.HtmlEncode(delimiter.ToString()));
                    }
                    continue;
                }

                if (command.Length == 1 && !char.IsLetter(command[0]))
                {
                    output.Append(WebUtility.HtmlEncode(command));
                    continue;
                }

                if (Symbols.TryGetValue(command, out string? symbol))
                {
                    output.Append(WebUtility.HtmlEncode(symbol));
                }
                else
                {
                    output.Append(WebUtility.HtmlEncode($"\\{command}"));
                }

                continue;
            }

            if (current == '{' || current == '}') continue;
            output.Append(WebUtility.HtmlEncode(current.ToString()));
        }

        return output.ToString();
    }

    private static string ReadCommand(string expression, ref int index)
    {
        if (++index >= expression.Length) return "\\";
        if (!char.IsLetter(expression[index])) return expression[index].ToString();

        int start = index;
        while (index + 1 < expression.Length && char.IsLetter(expression[index + 1])) index++;
        return expression[start..(index + 1)];
    }

    private static string ReadAtom(string expression, ref int index)
    {
        int next = index + 1;
        while (next < expression.Length && char.IsWhiteSpace(expression[next])) next++;
        if (next >= expression.Length)
        {
            index = expression.Length - 1;
            return string.Empty;
        }

        if (expression[next] == '{')
        {
            index = next;
            return ReadGroup(expression, ref index);
        }

        if (expression[next] == '\\')
        {
            int commandEnd = next + 1;
            if (commandEnd < expression.Length && char.IsLetter(expression[commandEnd]))
            {
                while (commandEnd + 1 < expression.Length && char.IsLetter(expression[commandEnd + 1])) commandEnd++;
            }

            index = commandEnd;
            return expression[next..(commandEnd + 1)];
        }

        index = next;
        return expression[next].ToString();
    }

    private static string ReadGroup(string expression, ref int index)
    {
        int next = index + 1;
        while (next < expression.Length && char.IsWhiteSpace(expression[next])) next++;
        if (next >= expression.Length)
        {
            index = expression.Length - 1;
            return string.Empty;
        }

        if (expression[next] != '{')
        {
            index = next;
            return ReadAtom(expression, ref index);
        }

        int start = next + 1;
        int depth = 1;
        for (int cursor = start; cursor < expression.Length; cursor++)
        {
            if (expression[cursor] == '{') depth++;
            else if (expression[cursor] == '}' && --depth == 0)
            {
                index = cursor;
                return expression[start..cursor];
            }
        }

        index = expression.Length - 1;
        return expression[start..];
    }

    private static void SkipOptionalGroup(string expression, ref int index)
    {
        int next = index + 1;
        while (next < expression.Length && char.IsWhiteSpace(expression[next])) next++;
        if (next >= expression.Length || expression[next] != '[') return;

        int depth = 1;
        for (int cursor = next + 1; cursor < expression.Length; cursor++)
        {
            if (expression[cursor] == '[') depth++;
            else if (expression[cursor] == ']' && --depth == 0)
            {
                index = cursor;
                return;
            }
        }

        index = expression.Length - 1;
    }
}
