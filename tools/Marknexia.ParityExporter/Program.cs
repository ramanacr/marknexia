namespace Marknexia.ParityExporter;

internal static class Program
{
    public static Task<int> Main(string[] args) => ParityExporterCli.RunAsync(args);
}

public static class ParityExporterCli
{
    public static async Task<int> RunAsync(string[] args, TextWriter? output = null, TextWriter? error = null)
    {
        output ??= Console.Out;
        error ??= Console.Error;
        try
        {
            string repositoryRoot = Directory.GetCurrentDirectory();
            string outputRoot = Path.Combine(repositoryRoot, "compat", "fixtures", "v1");
            bool verify = false;
            string? sourceRevision = null;

            for (int index = 0; index < args.Length; index++)
            {
                if (args[index] == "--output" && index + 1 < args.Length) outputRoot = Path.GetFullPath(args[++index]);
                else if (args[index] == "--verify") verify = true;
                // Regenerate while keeping the frozen oracle revision. The exporter still
                // refuses when the oracle/build allowlist differs from that revision.
                else if (args[index] == "--source-revision" && index + 1 < args.Length) sourceRevision = args[++index];
                else throw new ArgumentException($"Unknown argument '{args[index]}'.");
            }

            if (verify && sourceRevision is not null)
                throw new ArgumentException("--verify reads the frozen revision from the baseline; do not pass --source-revision.");

            if (verify)
            {
                await ParityBaselineVerifier.VerifyAsync(repositoryRoot, outputRoot, CancellationToken.None);
                await output.WriteLineAsync($"Verified frozen parity fixtures in {outputRoot} without modifying them.");
            }
            else
            {
                var exporter = new ParityExporter(repositoryRoot, sourceRevision);
                await exporter.ExportAsync(outputRoot, CancellationToken.None);
                await output.WriteLineAsync($"Exported parity fixtures to {outputRoot}.");
            }
            return 0;
        }
        catch (Exception exception)
        {
            await error.WriteLineAsync($"Parity exporter failed: {exception.Message}");
            return 1;
        }
    }
}
