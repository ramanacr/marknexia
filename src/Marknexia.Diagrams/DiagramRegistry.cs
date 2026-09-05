using Marknexia.Core;

namespace Marknexia.Diagrams;

public sealed class DiagramRegistry
{
    private readonly Dictionary<string, IDiagramRenderer> _renderers = new(StringComparer.OrdinalIgnoreCase);

    public DiagramRegistry()
    {
        Register(new MermaidDiagramRenderer());
    }

    public void Register(IDiagramRenderer renderer)
    {
        if (renderer == null) throw new ArgumentNullException(nameof(renderer));
        _renderers[renderer.DiagramType] = renderer;
    }

    public bool TryGetRenderer(string diagramType, out IDiagramRenderer? renderer)
    {
        return _renderers.TryGetValue(diagramType, out renderer);
    }

    public string RenderDiagram(string diagramType, string diagramSource, string diagramId)
    {
        if (TryGetRenderer(diagramType, out var renderer) && renderer != null)
        {
            return renderer.RenderDiagramToHtml(diagramSource, diagramId);
        }

        // Fallback for unregistered diagram types: render as code block
        return $@"<pre class=""diagram-unsupported""><code class=""language-{diagramType}"">{System.Net.WebUtility.HtmlEncode(diagramSource)}</code></pre>";
    }
}
