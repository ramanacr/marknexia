using System.Text.Json.Nodes;

namespace Marknexia.ParityExporter;

public sealed record ParityCase(
    string Area,
    string Name,
    JsonNode Input,
    JsonNode? VirtualFileSystem,
    JsonNode Expected,
    string SourceRevision)
{
    public const string SchemaVersion = "marknexia-parity-v1";
}
