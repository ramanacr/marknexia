namespace Marknexia.Files;

public sealed class RepositoryTreeNode
{
    public string DisplayName { get; }
    public string FullPath { get; }
    public bool IsFolder { get; }
    public string IconGlyph => IsFolder ? "\uE8B7" : "\uE8A5";
    public bool IsExpanded { get; set; }
    public IReadOnlyList<RepositoryTreeNode> Children { get; }

    public RepositoryTreeNode(string displayName, string fullPath, bool isFolder, IReadOnlyList<RepositoryTreeNode>? children = null)
    {
        DisplayName = displayName;
        FullPath = fullPath;
        IsFolder = isFolder;
        Children = children ?? Array.Empty<RepositoryTreeNode>();
    }
}
