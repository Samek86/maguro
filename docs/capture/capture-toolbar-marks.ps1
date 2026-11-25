# Toolbar and important-mark close-ups for README / help:
#   <lang>/toolbar_vertical.png    top window tabs, one pane; left strip with the icon column and More (>>)
#   <lang>/toolbar_horizontal.png  vertical window tabs, one pane; top strip with the title-row command bar and More (>>)
#   <lang>/feature_marks.png       lower part of a short window: two rows of 128px icons with three gold stars
#                                  and the status bar count; the cut top edge gets the same 1px frame line
# Seeds come from tabs*.json, tabs.vertical*.json and hero_light*.json in MAGURO_CFG_DIR (G: = demo drive).
# Env: MAGURO_PORTABLE, MAGURO_CFG_DIR, MAGURO_SHOT_DIR (images root; writes en/ ko/ ja/), MAGURO_FORCE=1
# Optional: MAGURO_LANGS (default en,ko,ja), MAGURO_KINDS (default toolbar_vertical,toolbar_horizontal,feature_marks)
# No input is sent: shots are taken right after startup. Only WM_CLOSE to the portable maguro.exe windows.
$ErrorActionPreference = 'Continue'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $here 'cap-lib.ps1')
$Portable = $env:MAGURO_PORTABLE; if (-not $Portable) { throw 'set MAGURO_PORTABLE' }
$CfgDir = $env:MAGURO_CFG_DIR; if (-not $CfgDir) { throw 'set MAGURO_CFG_DIR' }
$Out = $env:MAGURO_SHOT_DIR; if (-not $Out) { throw 'set MAGURO_SHOT_DIR' }
$Exe = Join-Path $Portable 'maguro.exe'
$Force = ($env:MAGURO_FORCE -eq '1')
$Langs = if ($env:MAGURO_LANGS) { $env:MAGURO_LANGS.Split(',') } else { @('en','ko','ja') }
$Kinds = if ($env:MAGURO_KINDS) { $env:MAGURO_KINDS.Split(',') } else { @('toolbar_vertical','toolbar_horizontal','feature_marks') }
$Seeds = Join-Path $env:TEMP ('maguro-tbm-seeds-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force $Seeds | Out-Null

# Window sizes (outer, 100% scale). Short / narrow on purpose so the toolbar sends the tail to More.
$py = @"
import json, sys, copy
from pathlib import Path
src = Path(r'$CfgDir'); dst = Path(r'$Seeds')
sizes = {'toolbar_vertical': [760, 430], 'toolbar_horizontal': [720, 460], 'feature_marks': [540, 460]}
def one_pane(path, view):
    return {'active': 0, 'columns': [1], 'colr': [1.0], 'rowr': [[1.0]]}, [{'index': 0, 'path': path, 'view': view}]
for lang, sfx in (('en', ''), ('ko', '.ko'), ('ja', '.ja')):
    for kind, base in (('toolbar_vertical', 'tabs%s.json'), ('toolbar_horizontal', 'tabs.vertical%s.json'), ('feature_marks', 'hero_light%s.json')):
        d = json.loads((src / (base % sfx)).read_text(encoding='utf-8-sig'))
        d['lang'] = lang; d['theme'] = 'light'; d['dark'] = False
        d['fav_panel'] = False; d['tree_panel'] = False
        w, h = sizes[kind]
        r = {'left': 60, 'top': 40, 'right': 60 + w, 'bottom': 40 + h, 'maximized': False}
        d['window'] = r
        for x in d.get('windows', []): x['window'] = dict(r)
        if kind == 'feature_marks':
            lay, panes = one_pane('G:\\Mixed', [1, 128])
            t = d['tabs'][0]; t['name'] = 'Mixed'
            t['layout'] = copy.deepcopy(lay); t['panes'] = copy.deepcopy(panes); d['tabs'] = [t]
            for x in d.get('windows', []): x['tabs'] = copy.deepcopy(d['tabs'])
        else:
            lay, panes = one_pane('G:\\Photos 2026', [1, 96])
            for t in [d['tabs'][0]] + [x['tabs'][0] for x in d.get('windows', [])]:
                t['layout'] = copy.deepcopy(lay); t['panes'] = copy.deepcopy(panes)
        d['layout'] = lay; d['panes'] = panes
        (dst / ('%s.%s.json' % (kind, lang))).write_text(json.dumps(d, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
"@
$tmpPy = Join-Path $Seeds 'seed.py'
Set-Content -Path $tmpPy -Value $py -Encoding utf8
python $tmpPy
if ($LASTEXITCODE -ne 0) { throw 'seed build failed' }

# Three marks in G:\Mixed: a folder and two files on the first two rows of the 128px icon grid.
$MarksJson = @'
{"version":1,"marks":[
{"path":"G:\\Mixed\\Archive","dir":true,"vol":null,"fid":null,"state":"ok","added":"2026-11-20T00:00:00Z"},
{"path":"G:\\Mixed\\app.ts","dir":false,"vol":null,"fid":null,"state":"ok","added":"2026-11-20T00:00:00Z"},
{"path":"G:\\Mixed\\budget-2026.csv","dir":false,"vol":null,"fid":null,"state":"ok","added":"2026-11-20T00:00:00Z"}
]}
'@
# Every shot is a window capture (white backdrop + DWM bounds) cut down to the part it shows.
# Values are window pixels at 96 DPI. Edges that stay are the window's own 1px frame; each cut edge
# gets a 1px line in the frame colour read from the window edge on the same side (middle pixel).
#   toolbar_vertical    left 140 px, full height: icon column with More (>>) and Change, plus the pane edge
#   toolbar_horizontal  top 95 px, full width: title row command bar through More and Change, the right
#                       buttons, and the tab list / address row under it
#   feature_marks       drops the top 116 px (title, tabs, address, column headers)
$Crops = @{
  toolbar_vertical   = @{ Top = 0;   Width = 140; Height = 0 }
  toolbar_horizontal = @{ Top = 0;   Width = 0;   Height = 95 }
  feature_marks      = @{ Top = 116; Width = 0;   Height = 0 }
}

function Save-WindowCrop([string]$src, [string]$dest, [IntPtr]$h, $c) {
  $s = [MaguroCap]::Dpi($h) / 96.0
  $img = [Drawing.Bitmap]::FromFile($src)
  $W = $img.Width; $H = $img.Height
  $top = [int]($c.Top * $s)
  $cw = if ($c.Width -gt 0) { [Math]::Min($W, [int]($c.Width * $s)) } else { $W }
  $ch = if ($c.Height -gt 0) { [Math]::Min($H - $top, [int]($c.Height * $s)) } else { $H - $top }
  $right = $img.GetPixel($W - 1, [int]($H / 2))
  $bottom = $img.GetPixel([int]($W / 2), $H - 1)
  $bmp = New-Object Drawing.Bitmap $cw, $ch
  $g = [Drawing.Graphics]::FromImage($bmp)
  $g.DrawImage($img, (New-Object Drawing.Rectangle 0, 0, $cw, $ch), (New-Object Drawing.Rectangle 0, $top, $cw, $ch), [Drawing.GraphicsUnit]::Pixel)
  $img.Dispose()
  $g.Dispose()
  # Draw the cut-edge lines pixel by pixel so no anti-aliasing touches them.
  if ($top -gt 0) { for ($x = 0; $x -lt $cw; $x++) { $bmp.SetPixel($x, 0, $bottom) } }
  if ($cw -lt $W) { for ($y = 0; $y -lt $ch; $y++) { $bmp.SetPixel($cw - 1, $y, $right) } }
  if (($top + $ch) -lt $H) { for ($x = 0; $x -lt $cw; $x++) { $bmp.SetPixel($x, $ch - 1, $bottom) } }
  $e = Test-MaguroEdges $bmp
  if (-not $e.Ok) { $bmp.Dispose(); throw "crop edge check failed $dest" }
  $bmp.Save($dest, [Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
  Write-Host ("OK-crop {0} {1}x{2}" -f $dest, $cw, $ch)
}

$marksPath = Join-Path $Portable 'marks.json'
foreach ($lang in $Langs) {
  foreach ($kind in $Kinds) {
    if (Test-MaguroCaptureStop) { break }
    $dest = Join-Path (Join-Path $Out $lang) "$kind.png"
    if ((Test-Path $dest) -and -not $Force) { Write-Host "skip $dest"; continue }
    New-Item -ItemType Directory -Force (Split-Path $dest -Parent) | Out-Null
    Write-Host "=== $lang/$kind ==="
    Close-PortableMaguro $Portable; Close-MaguroBackdrop
    Copy-Item -Force (Join-Path $Seeds "$kind.$lang.json") (Join-Path $Portable 'maguro.json')
    if ($kind -eq 'feature_marks') { Set-Content -Path $marksPath -Value $MarksJson -Encoding utf8 }
    else { Remove-Item $marksPath -Force -EA SilentlyContinue }
    $p = Start-Process $Exe -WorkingDirectory $Portable -PassThru
    $h = [IntPtr]::Zero; $deadline = (Get-Date).AddSeconds(20)
    do { $h = [MaguroCap]::FindCls([uint32]$p.Id, 'MaguroMain'); if ($h -ne [IntPtr]::Zero) { break }; Start-Sleep -Milliseconds 200 } while ((Get-Date) -lt $deadline)
    if ($h -eq [IntPtr]::Zero) { Write-Host "FAIL no MaguroMain $lang/$kind"; continue }
    Start-Sleep -Milliseconds 2500   # thumbnails and star overlays
    try {
      $full = Join-Path $Seeds "full-$kind-$lang.png"
      Cap-MaguroWindow $h $full
      Save-WindowCrop $full $dest $h $Crops[$kind]
    } catch { Write-Host "FAIL $lang/$kind : $_" }
    [void][MaguroCap]::PostMessage($h, 0x0010, [UIntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 600
    Close-PortableMaguro $Portable
  }
}
Remove-Item $marksPath -Force -EA SilentlyContinue
Remove-Item $Seeds -Recurse -Force -EA SilentlyContinue
Close-MaguroBackdrop
Write-Host 'capture-toolbar-marks DONE'
