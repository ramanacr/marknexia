namespace Marknexia.ParityExporter;

internal static class Program
{
    public static async Task<int> Main(string[] args)
    {
        string repositoryRoot = Directory.GetCurrentDirectory();
        string outputRoot = Path.Combine(repositoryRoot, "compat", "fixtures", "v1");
        bool verify = false;

        for (int index = 0; index < args.Length; index++)
        {
            if (args[index] == "--output" && index + 1 < args.Length) outputRoot = Path.GetFullPath(args[++index]);
            else if (args[index] == "--verify") verify = true;
            else throw new ArgumentException($"Unknown argument '{args[index]}'.");
        }

        var exporter = new ParityExporter(repositoryRoot);
        await exporter.ExportAsync(outputRoot, CancellationToken.None);
        if (verify) Console.WriteLine($"Validated deterministic parity fixtures in {outputRoot}.");
        return 0;
    }
}
