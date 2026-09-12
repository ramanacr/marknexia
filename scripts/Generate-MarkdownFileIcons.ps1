[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$OutputDirectory
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Drawing.Common -ErrorAction SilentlyContinue

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$size = 64
$bitmap = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
try {
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.Clear([System.Drawing.Color]::Transparent)
    $paper = [System.Drawing.Point[]]@(
        [System.Drawing.Point]::new(10, 5), [System.Drawing.Point]::new(42, 5),
        [System.Drawing.Point]::new(54, 17), [System.Drawing.Point]::new(54, 59),
        [System.Drawing.Point]::new(10, 59))
    $graphics.FillPolygon([System.Drawing.Brushes]::White, $paper)
    $graphics.DrawPolygon([System.Drawing.Pens]::LightSlateGray, $paper)
    $fold = [System.Drawing.Point[]]@(
        [System.Drawing.Point]::new(42, 5), [System.Drawing.Point]::new(42, 17),
        [System.Drawing.Point]::new(54, 17))
    $graphics.FillPolygon([System.Drawing.Brushes]::LightSteelBlue, $fold)
    $graphics.DrawPolygon([System.Drawing.Pens]::SteelBlue, $fold)
    $graphics.FillRectangle([System.Drawing.Brushes]::RoyalBlue, 16, 28, 32, 19)
    $font = [System.Drawing.Font]::new("Segoe UI", 12, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    try { $graphics.DrawString("M", $font, [System.Drawing.Brushes]::White, 23, 29) }
    finally { $font.Dispose() }

    $pngPath = Join-Path $OutputDirectory "MarkdownFileLogo.png"
    $pngStream = [System.IO.MemoryStream]::new()
    try {
        $bitmap.Save($pngStream, [System.Drawing.Imaging.ImageFormat]::Png)
        $pngBytes = $pngStream.ToArray()
        [System.IO.File]::WriteAllBytes($pngPath, $pngBytes)
    }
    finally { $pngStream.Dispose() }

    $icoPath = Join-Path $OutputDirectory "markdown-file.ico"
    $icoStream = [System.IO.MemoryStream]::new()
    $writer = [System.IO.BinaryWriter]::new($icoStream)
    try {
        $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]1)
        $writer.Write([byte]64); $writer.Write([byte]64); $writer.Write([byte]0); $writer.Write([byte]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32); $writer.Write([uint32]$pngBytes.Length); $writer.Write([uint32]22)
        $writer.Write($pngBytes)
        [System.IO.File]::WriteAllBytes($icoPath, $icoStream.ToArray())
    }
    finally { $writer.Dispose(); $icoStream.Dispose() }
}
finally {
    $graphics.Dispose()
    $bitmap.Dispose()
}
