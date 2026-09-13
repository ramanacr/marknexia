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

            for (int index = 0; index < args.Length; index++)
            {
                if (args[index] == "--output" && index + 1 < args.Length) outputRoot = Path.GetFullPath(args[++index]);
                else if (args[index] == "--verify") verify = true;
                else throw new ArgumentException($"Unknown argument '{args[index]}'.");
            }

            if (verify)
            {
                await ParityBaselineVerifier.VerifyAsync(repositoryRoot, outputRoot, CancellationToken.None);
                await output.WriteLineAsync($"Verified frozen parity fixtures in {outputRoot} without modifying them.");
            }
            else
            {
                var exporter = new ParityExporter(repositoryRoot);
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
