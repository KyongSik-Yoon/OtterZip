#requires -Version 5.1
# Generate per-format file icons: the shell mascot with a large, bottom-placed
# extension label (ZIP / 7Z / RAR / ...) so users tell archive variants apart
# at a glance in Explorer — WinRAR / 7-Zip convention.
#
# Design ("Variant C2/LT", approved 2026-07):
#   * NO badge plate. The extension is drawn as large text with a dark outline
#     (halo) so it reads on the cream shell AND on any Explorer background.
#   * Text is color-coded per format family and placed toward the BOTTOM.
#   * Label sized via GenericTypographic + cap-height fudge so it fills nearly
#     the whole icon width (plain auto-fit rendered it ~40% smaller / too small).
#   * Small sizes (<=32px): the extension DOMINATES — the shell shrinks and
#     fades to a faint backdrop so 3-4 characters stay legible at 16px (the
#     old centered-badge design was invisible at these sizes).
#   * Large sizes (>32px): full shell mascot + a big bottom label.
#
# Output layout (per format key like "zip", "7z", ...):
#   Assets/FileIcons/{key}/Icon.targetsize-{N}.png
# Package.appxmanifest has one <uap:FileTypeAssociation> per format key.

param(
    [string]$ShellSrc = "D:\11.AI\SpanZIP\app\OtterZip.App\Assets\ShellMascot.png",
    [string]$OutRoot = "D:\11.AI\SpanZIP\app\OtterZip.App\Assets\FileIcons",
    # Comma-separated format keys to (re)generate, e.g. "zip,7z,rar". Default = all.
    [string]$Only = "",
    [switch]$SkipExisting
)

Add-Type -AssemblyName System.Drawing

# Format key -> on-icon label. Kept <=4 chars; the fitter auto-shrinks 4-char
# labels (TBZ2 / TZST / LZMA / ZIPX) so they never clip.
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

# Dark warm-brown outline drawn under the colored fill — gives every glyph a
# halo that keeps it legible on light Explorer backgrounds.
$outlineColor = [System.Drawing.Color]::FromArgb(255, 30, 20, 10)

# Per-format label color, grouped by family so each archive class reads at a
# glance even when the glyphs are too small to spell out.
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

$shellBmp = [System.Drawing.Bitmap]::FromFile($ShellSrc)
Write-Host ("Source shell: {0}x{1}" -f $shellBmp.Width, $shellBmp.Height)

function New-Canvas([int]$s) {
    $bmp = New-Object System.Drawing.Bitmap $s, $s, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode      = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.InterpolationMode  = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode    = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.TextRenderingHint  = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $g.Clear([System.Drawing.Color]::Transparent)
    return @($bmp, $g)
}

# Draw the shell, optionally scaled and alpha-faded, top- or center-aligned.
function Draw-Shell($g, [int]$s, [double]$scale, [double]$alpha, [string]$valign) {
    $w = [int]($s * $scale)
    $x = [int](($s - $w) / 2)
    if ($valign -eq 'top') { $y = 0 } else { $y = [int](($s - $w) / 2) }
    $rect = New-Object System.Drawing.Rectangle $x, $y, $w, $w
    if ($alpha -ge 0.999) {
        $g.DrawImage($shellBmp, $rect)
    } else {
        $cm = New-Object System.Drawing.Imaging.ColorMatrix
        $cm.Matrix33 = [single]$alpha
        $ia = New-Object System.Drawing.Imaging.ImageAttributes
        $ia.SetColorMatrix($cm)
        $g.DrawImage($shellBmp, $rect, 0, 0, $shellBmp.Width, $shellBmp.Height, [System.Drawing.GraphicsUnit]::Pixel, $ia)
        $ia.Dispose()
    }
}

# Fit + draw an outlined, centered label. wF/hF are box fractions of the canvas,
# cy is the box's vertical-center fraction, outFactor scales the halo pen width.
function Draw-Text($g, [int]$s, [string]$label, $fillCol, $outCol,
                   [double]$wF, [double]$hF, [double]$cy, [double]$outFactor, [double]$fudge) {
    $fam = New-Object System.Drawing.FontFamily "Segoe UI"
    # GenericTypographic removes the glyph side-bearing so the label can grow to
    # nearly the full icon width. The fudge factor compensates for MeasureString's
    # tall line box (leading/descent) so the visible cap-height fills the box —
    # a plain fit left the glyph at ~53% of the box and looked small.
    $sf = [System.Drawing.StringFormat]::GenericTypographic.Clone()
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $sf.FormatFlags = $sf.FormatFlags -bor [System.Drawing.StringFormatFlags]::NoClip
    $boxW = $s * $wF; $boxH = $s * $hF
    $boxX = ($s - $boxW) / 2; $boxY = $s * $cy - $boxH / 2
    $rect = New-Object System.Drawing.RectangleF ([single]$boxX), ([single]$boxY), ([single]$boxW), ([single]$boxH)
    $refPx = 100.0
    $rf = New-Object System.Drawing.Font $fam, $refPx, ([System.Drawing.FontStyle]::Bold)
    $area = New-Object System.Drawing.SizeF ([single]10000), ([single]10000)
    $m = $g.MeasureString($label, $rf, $area, $sf); $rf.Dispose()
    $byW = $refPx * $boxW / $m.Width
    $byH = $refPx * $boxH / $m.Height * $fudge
    $fontPx = [Math]::Max([Math]::Min($byW, $byH), 4)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddString($label, $fam, [int]([System.Drawing.FontStyle]::Bold), [single]$fontPx, $rect, $sf)
    $penW = [Math]::Max($fontPx * $outFactor, 1.0)
    $pen = New-Object System.Drawing.Pen $outCol, ([single]$penW)
    $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $g.DrawPath($pen, $path)
    $brush = New-Object System.Drawing.SolidBrush $fillCol
    $g.FillPath($brush, $path)
    $pen.Dispose(); $brush.Dispose(); $path.Dispose(); $sf.Dispose(); $fam.Dispose()
}

$count = 0
foreach ($f in $formats) {
    $dstDir = Join-Path $OutRoot $f.key
    if (-not (Test-Path $dstDir)) { New-Item -ItemType Directory -Path $dstDir | Out-Null }
    $fill = Get-FormatFill $f.key

    foreach ($s in $sizes) {
        $dstPath = Join-Path $dstDir ("Icon.targetsize-{0}.png" -f $s)
        if ($SkipExisting -and (Test-Path $dstPath)) { continue }

        $pair = New-Canvas $s
        $bmp = $pair[0]; $g = $pair[1]

        # Variant "LT" params — biggest legible label (typographic + cap fudge).
        if ($s -le 32) {
            # Small: extension dominates; shell is a faint top backdrop.
            Draw-Shell $g $s 0.50 0.20 'top'
            $wF = 1.00; $hF = 0.90; $bottomPad = 0.03; $outFactor = 0.10; $fudge = 1.42
        } else {
            # Large: full shell + big bottom label.
            Draw-Shell $g $s 1.00 1.00 'center'
            $wF = 0.96; $hF = 0.66; $bottomPad = 0.05; $outFactor = 0.15; $fudge = 1.30
        }
        $cy = (1.0 - $bottomPad) - $hF / 2.0
        Draw-Text $g $s $f.label $fill $outlineColor $wF $hF $cy $outFactor $fudge

        $g.Dispose()
        $bmp.Save($dstPath, [System.Drawing.Imaging.ImageFormat]::Png)
        $bmp.Dispose()
        $count++
    }
    Write-Host ("  {0,-6} -> {1}/" -f $f.label, $f.key)
}

$shellBmp.Dispose()
Write-Host ("`nGenerated {0} PNG files across {1} formats x {2} sizes" -f $count, $formats.Count, $sizes.Count)
