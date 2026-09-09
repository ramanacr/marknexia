using System.Net;
using Marknexia.Core;

namespace Marknexia.Diagrams;

public sealed class MermaidDiagramRenderer : IDiagramRenderer
{
    public string DiagramType => "mermaid";

    public string RenderDiagramToHtml(string diagramSource, string diagramId)
    {
        string encodedSource = WebUtility.HtmlEncode(diagramSource);
        string uriEscaped = Uri.EscapeDataString(diagramSource);

        return $@"
<div class=""marknexia-diagram marknexia-mermaid"" id=""{diagramId}"" data-diagram-type=""mermaid"">
  <div class=""marknexia-diagram-toolbar"">
    <span class=""marknexia-diagram-label"">Mermaid Diagram</span>
    <div class=""marknexia-diagram-actions"">
      <button type=""button"" class=""marknexia-btn marknexia-btn-copy"" data-marknexia-action=""copy"" data-copy-text=""{uriEscaped}"" title=""Copy Diagram Source"">
        Copy
      </button>
      <button type=""button"" class=""marknexia-btn marknexia-btn-toggle"" data-marknexia-action=""toggle-source"" title=""Toggle Diagram Source"">
        Source
      </button>
    </div>
  </div>
  <div class=""marknexia-diagram-viewport"" id=""{diagramId}-viewport"">
    <pre class=""mermaid"" id=""{diagramId}-render"">{encodedSource}</pre>
  </div>
  <div class=""marknexia-diagram-source"" id=""{diagramId}-source"" style=""display: none;"">
    <pre><code>{encodedSource}</code></pre>
  </div>
  <div class=""marknexia-diagram-error"" id=""{diagramId}-error"" style=""display: none;"">
    <div class=""marknexia-diagram-error-title"">Diagram failed to render</div>
    <div class=""marknexia-diagram-error-msg"" id=""{diagramId}-error-msg""></div>
    <div class=""marknexia-diagram-error-source"">
      <pre><code>{encodedSource}</code></pre>
    </div>
  </div>
</div>";
    }
}
