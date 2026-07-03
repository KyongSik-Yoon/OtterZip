#requires -Version 5.1
# Generate per-format file icons with format-name badges (ZIP / 7Z / RAR / ...)
# composited over the shell mascot. Mirrors the WinRAR / 7-Zip convention so
# users can tell archive variants apart at a glance in Explorer.
#
# Layout:
#   Top  ~70% : shell mascot (ShellMascot.png)
#   Bot  ~30% : solid brown badge with white bold sans-serif label
#
# Sizes < 48px: badge would be unreadable, so we ship a bare-shell icon
# (mirrors the original ArchiveIcon). Above 48px, the label is rendered.
#
# Output layout (for each format key like "zip", "7z", ...):
#   Assets/FileIcons/{key}/Icon.targetsize-{N}.png
#
# The Package.appxmanifest then has one <uap:FileTypeAssociation> per
# format key, pointing Logo at "Assets\FileIcons\{key}\Icon.png" - MRT
# resolves the .targetsize-N qualifier at runtime.

param(
    [string]$ShellSrc = "D:\11.AI\SpanZIP\app\OtterZip.App\Assets\ShellMascot.png",
    [string]$OutRoot = "D:\11.AI\SpanZIP\app\OtterZip.App\Assets\FileIcons",
    # Comma-separated list of format keys to generate, e.g. "zip,7z,rar".
    # Default = all 29 formats wired in Package.appxmanifest.
    [string]$Only = "",
    # Set to skip already-rendered icons (idempotent re-runs).
    [switch]$SkipExisting
)

Add-Type -AssemblyName System.Drawing

# Format key -> on-icon label text. Labels are kept <=4 chars so they
# fit at 48px without forcing an unreadably small font.
$formats = @(
    @{ key = 'zip';  label = 'ZIP'  },
    @{ key = 'zipx'; label = 'ZIPX' },
    @{ key = '7z';   label = '7Z'   },
    @{ key = 'rar';  label = 'RAR'  },
    @{ key = 'tar';  label = 'TAR'  },
    @{ key = 'tgz';  label = 'TGZ'  },
    @{ key = 'tbz';  label = 'TBZ'  },
    @{ key = 'tbz2'; label = 'TBZ2' },
    @{ key = 'tlz';  label = 'TLZ'  },
    @{ key = 'txz';  label = 'TXZ'  },
    @{ key = 'tzst'; label = 'TZST' },
    @{ key = 'gz';   label = 'GZ'   },
    @{ key = 'bz2';  label = 'BZ2'  },
    @{ key = 'xz';   label = 'XZ'   },
    @{ key = 'lzma'; label = 'LZMA' },
    @{ key = 'zst';  label = 'ZST'  },
    @{ key = 'lz4';  label = 'LZ4'  },
    @{ key = 'jar';  label = 'JAR'  },
    @{ key = 'war';  label = 'WAR'  },
    @{ key = 'ear';  label = 'EAR'  },
    @{ key = 'ipa';  label = 'IPA'  },
    @{ key = 'apk';  label = 'APK'  },
    @{ key = 'aab';  label = 'AAB'  },
    @{ key = 'xpi';  label = 'XPI'  },
    @{ key = 'crx';  label = 'CRX'  },
    @{ key = 'iso';  label = 'ISO'  },
    @{ key = 'img';  label = 'IMG'  },
    @{ key = 'cab';  label = 'CAB'  },
    @{ key = 'deb';  label = 'DEB'  }
)

if ($Only -ne "") {
    $allowed = $Only.Split(',') | ForEach-Object { $_.Trim() }
    $formats = $formats | Where-Object { $allowed -contains $_.key }
    Write-Host ("Filter: rendering only {0}" -f ($formats.key -join ', '))
}

$sizes = @(16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256)
# Overlay design: shell always renders at FULL canvas size; badge is
# overlaid on top, covering only the bottom strip. Works at every size
# (Bandizip-style) because the shell never shrinks. At very small sizes
# the text becomes a tag-like color band (legibility falls off below
# ~32px but the colored band still differentiates).
$labelMinSize = 0    # 0 = render badge at every size

$shellBmp = [System.Drawing.Bitmap]::FromFile($ShellSrc)
Write-Host ("Source shell: {0}x{1}" -f $shellBmp.Width, $shellBmp.Height)

# Thick label outline — near-black warm brown. The colour fill is drawn
# ON TOP, so a Pen of width W leaves a visible outer halo of ~W/2; this
# keeps the glyph legible on the shell and on any Explorer background.
$outlineColor = [System.Drawing.Color]::FromArgb(255, 38, 24, 12)

# Per-format label fill colour, grouped by family so each archive class
# (ZIP / 7z / RAR / TAR / stream-compressor / disk-image / sys-package)
# reads at a glance even when the glyphs are too small to spell out.
function Get-FormatFill([string]$key) {
    switch -Regex ($key) {
        '^(zip|zipx|jar|war|ear|ipa|apk|aab|xpi|crx)$' { return [System.Drawing.Color]::FromArgb(255, 244, 168, 0) }
        '^7z$'   { return [System.Drawing.Color]::FromArgb(255, 240, 106, 42) }
        '^rar$'  { return [System.Drawing.Color]::FromArgb(255, 186, 96, 216) }
        '^(tar|tgz|tbz|tbz2|tlz|txz|tzst)$' { return [System.Drawing.Color]::FromArgb(255, 206, 142, 78) }
        '^(gz|bz2|xz|lzma|zst|lz4)$' { return [System.Drawing.Color]::FromArgb(255, 46, 198, 172) }
        '^(iso|img)$' { return [System.Drawing.Color]::FromArgb(255, 82, 160, 236) }
        '^(cab|deb)$' { return [System.Drawing.Color]::FromArgb(255, 150, 158, 178) }
        default { return [System.Drawing.Color]::White }
    }
}

$count = 0
foreach ($f in $formats) {
    $dstDir = Join-Path $OutRoot $f.key
    if (-not (Test-Path $dstDir)) { New-Item -ItemType Directory -Path $dstDir | Out-Null }

    foreach ($s in $sizes) {
        $dstPath = Join-Path $dstDir ("Icon.targetsize-{0}.png" -f $s)
        if ($SkipExisting -and (Test-Path $dstPath)) { continue }

        $bmp = New-Object System.Drawing.Bitmap $s, $s, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.SmoothingMode      = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
        $g.InterpolationMode  = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.PixelOffsetMode    = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $g.TextRenderingHint  = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
        $g.Clear([System.Drawing.Color]::Transparent)

        # Shell at FULL canvas size — mascot stays prominent.
        $g.DrawImage($shellBmp, (New-Object System.Drawing.Rectangle 0, 0, $s, $s))

        if ($s -ge $labelMinSize) {
            # Pre-1.0 icon redesign: a colored-glyph-on-shell label washed
            # out (amber/brown text on the cream shell was near-invisible at
            # small sizes). Industry convention (Bandizip / Windows): a solid
            # per-format-color BADGE PLATE at the bottom with WHITE bold
            # text — maximum contrast at every size, and the plate color
            # still encodes the format family. The plate is lifted off the
            # bottom edge so it doesn't read as clipped in Explorer grids.
            $fontFamily = New-Object System.Drawing.FontFamily "Segoe UI"
            $sf = New-Object System.Drawing.StringFormat
            $sf.Alignment = [System.Drawing.StringAlignment]::Center
            $sf.LineAlignment = [System.Drawing.StringAlignment]::Center

            # Small icons: the plate takes ~52% of the height and the full
            # width — at 16-40px the extension is the only thing that can
            # read, so legibility beats shell visibility. Larger icons keep
            # the mascot dominant with a slimmer plate.
            if ($s -le 40) {
                $plateH = $s * 0.52
                $plateInset = 0.0
                $bottomPad = [Math]::Max($s * 0.03, 1)
                $textWFactor = 0.98; $textHFactor = 0.86
            } else {
                $plateH = $s * 0.40
                $plateInset = $s * 0.02
                $bottomPad = $s * 0.05
                $textWFactor = 0.92; $textHFactor = 0.80
            }
            $plateY = $s - $plateH - $bottomPad
            $plateW = $s - (2 * $plateInset)
            $radius = [Math]::Max($plateH * 0.28, 2.0)

            # Rounded-rect plate in the format-family color with a subtle
            # dark border so it also reads on white Explorer backgrounds.
            $plate = New-Object System.Drawing.Drawing2D.GraphicsPath
            $d = $radius * 2
            $plate.AddArc([single]$plateInset, [single]$plateY, [single]$d, [single]$d, 180, 90)
            $plate.AddArc([single]($plateInset + $plateW - $d), [single]$plateY, [single]$d, [single]$d, 270, 90)
            $plate.AddArc([single]($plateInset + $plateW - $d), [single]($plateY + $plateH - $d), [single]$d, [single]$d, 0, 90)
            $plate.AddArc([single]$plateInset, [single]($plateY + $plateH - $d), [single]$d, [single]$d, 90, 90)
            $plate.CloseFigure()

            $plateBrush = New-Object System.Drawing.SolidBrush (Get-FormatFill $f.key)
            $g.FillPath($plateBrush, $plate)
            $plateBrush.Dispose()
            $borderW = [Math]::Max([int][Math]::Round($s * 0.02), 1)
            $borderPen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(110, 38, 24, 12)), $borderW
            $g.DrawPath($borderPen, $plate)
            $borderPen.Dispose()
            $plate.Dispose()

            # White bold label centered on the plate, auto-fitted so 4-char
            # labels (TBZ2 / TZST) never clip.
            $rect = New-Object System.Drawing.RectangleF ([single]$plateInset), ([single]$plateY), ([single]$plateW), ([single]$plateH)
            $refPx = 100.0
            $refFont = New-Object System.Drawing.Font $fontFamily, $refPx, ([System.Drawing.FontStyle]::Bold)
            $meas = $g.MeasureString($f.label, $refFont)
            $refFont.Dispose()
            $fitW = ($plateW * $textWFactor) / $meas.Width
            $fitH = ($plateH * $textHFactor) / $meas.Height
            $fontPx = [Math]::Max($refPx * [Math]::Min($fitW, $fitH), 4)

            $path = New-Object System.Drawing.Drawing2D.GraphicsPath
            $path.AddString(
                $f.label,
                $fontFamily,
                [int]([System.Drawing.FontStyle]::Bold),
                [single]$fontPx,
                $rect,
                $sf
            )
            $textBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
            $g.FillPath($textBrush, $path)
            $textBrush.Dispose()
            $path.Dispose()

            $sf.Dispose()
            $fontFamily.Dispose()
        }

        $g.Dispose()
        $bmp.Save($dstPath, [System.Drawing.Imaging.ImageFormat]::Png)
        $bmp.Dispose()
        $count++
    }
    Write-Host ("  {0,-6} -> {1}/" -f $f.label, $f.key)
}

$shellBmp.Dispose()
Write-Host ("`nGenerated {0} PNG files across {1} formats x {2} sizes" -f $count, $formats.Count, $sizes.Count)
