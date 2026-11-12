# HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
# SPDX-License-Identifier: AGPL-3.0-only
#
# End-to-end smoke test: engine self-test, command line round trips, both renderers, performance.
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1 [-Exe target\release\hexiso.exe] [-SkipBuild] [-Mount]
#   -Mount  also mounts / unmounts a test image (changes drive letters for a moment).
# Writes everything to a fresh folder under %TEMP%\hexiso-smoke and prints PASS / FAIL per check.
param([string]$Exe = "", [switch]$SkipBuild, [switch]$Mount)

$ErrorActionPreference = "Continue"
Set-Location (Split-Path $PSScriptRoot -Parent)
if (-not $SkipBuild) {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] cargo build --release"; exit 1 }
}
if ($Exe -eq "") { $Exe = (Resolve-Path "target\release\hexiso.exe").Path }
$work = Join-Path $env:TEMP ("hexiso-smoke\" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$script:fails = 0
function Check($name, [scriptblock]$test) {
    try {
        $r = & $test
        if ($r -is [string] -and $r.StartsWith("FAIL")) { $script:fails++; Write-Host ("[FAIL] {0,-34} {1}" -f $name, $r) }
        else { Write-Host ("[ ok ] {0,-34} {1}" -f $name, $r) }
    } catch { $script:fails++; Write-Host ("[FAIL] {0,-34} {1}" -f $name, $_.Exception.Message) }
}
function Hx { & $Exe @args 2>$null }

# --- engine self-test (builds, edits, converts, encrypts, splits, re-reads) ---
Check "engine selftest" { $o = & $Exe selftest (Join-Path $work "st") 2>&1 | Out-String; if ($LASTEXITCODE -eq 0 -and $o -match "ALL PASSED") { "all checks passed" } else { "FAIL`n$o" } }

# --- sample data ---
$src = Join-Path $work "src"
New-Item -ItemType Directory -Force (Join-Path $src "docs\deep") | Out-Null
Set-Content -Encoding ascii (Join-Path $src "readme.txt") "hello hexiso"
[IO.File]::WriteAllBytes((Join-Path $src "docs\deep\blob.bin"), [byte[]](1..200000 | ForEach-Object { $_ % 251 }))
# Unicode file name built from code points (Windows PowerShell 5.1 reads scripts as ANSI)
$uname = "na$([char]0xEF)ve $([char]0x2013) $([char]0xFC)n$([char]0xEF)c$([char]0xF6)d$([char]0xE9).txt"
Set-Content -Encoding utf8 -LiteralPath (Join-Path $src "docs\$uname") "unicode name"
$iso = Join-Path $work "t.iso"

# --- command line ---
Check "version / help"           { $v = Hx version; $h = Hx help | Out-String; if ($v -match "hexiso" -and $h -match "USAGE") { $v } else { "FAIL" } }
Check "create"                   { Hx create $iso (Join-Path $src "readme.txt") (Join-Path $src "docs") --label SMOKE -q | Out-Null; if ($LASTEXITCODE -eq 0 -and (Test-Path $iso)) { "{0} bytes" -f (Get-Item $iso).Length } else { "FAIL exit $LASTEXITCODE" } }
Check "info --json"              { $j = Hx info $iso --json | ConvertFrom-Json; if ($j.label -eq "SMOKE" -and $j.files -eq 3) { "label $($j.label), $($j.files) files, fs $($j.filesystem)" } else { "FAIL $($j | ConvertTo-Json -Compress)" } }
Check "ls -r --json"             { $l = Hx ls $iso -r --json | ConvertFrom-Json; if (($l | Where-Object path -eq "docs/deep/blob.bin").size -eq 200000) { "$($l.Count) entries" } else { "FAIL" } }
Check "add --to / rename / rm"   {
    Hx add $iso (Join-Path $src "readme.txt") --to extra/sub -q | Out-Null
    Hx rename $iso readme.txt READ.ME -q | Out-Null
    Hx rm $iso "extra/sub/readme.txt" -q | Out-Null
    $p = (Hx ls $iso -r --json | ConvertFrom-Json).path
    if (($p -contains "READ.ME") -and ($p -contains "extra/sub") -and -not ($p -contains "extra/sub/readme.txt")) { "edits saved" } else { "FAIL $($p -join ',')" }
}
Check "label + boot"             { Hx label $iso NEWLABEL -q | Out-Null; Hx boot $iso --bios READ.ME -q | Out-Null; $j = Hx info $iso --json | ConvertFrom-Json; if ($j.label -eq "NEWLABEL" -and $j.boot.Count -eq 1) { "label + BIOS boot entry" } else { "FAIL" } }
Check "extract + compare"        {
    $out = Join-Path $work "out"; Hx extract $iso $out -q | Out-Null
    $a = (Get-FileHash (Join-Path $src "docs\deep\blob.bin")).Hash; $b = (Get-FileHash (Join-Path $out "docs\deep\blob.bin")).Hash
    $u = Test-Path -LiteralPath (Join-Path $out "docs\$uname")
    if ($a -eq $b -and $u) { "bytes identical, unicode name kept" } else { "FAIL hash=$($a -eq $b) unicode=$u" }
}
Check "convert cso / hxz (+pw)"  {
    Hx convert $iso (Join-Path $work "t.cso") -q | Out-Null
    Hx convert $iso (Join-Path $work "t.hxz") --encrypt pw -q | Out-Null
    Hx info (Join-Path $work "t.hxz") -q 2>$null | Out-Null; $noPw = $LASTEXITCODE
    $c1 = (Hx checksum $iso --in docs/deep/blob.bin --json | ConvertFrom-Json).sha256
    $c2 = (Hx checksum (Join-Path $work "t.cso") --in docs/deep/blob.bin --json | ConvertFrom-Json).sha256
    $c3 = (Hx checksum (Join-Path $work "t.hxz") --in docs/deep/blob.bin --password pw --json | ConvertFrom-Json).sha256
    if ($noPw -eq 1 -and $c1 -eq $c2 -and $c1 -eq $c3) { "same content in all three; password enforced" } else { "FAIL nopw=$noPw $c1 $c2 $c3" }
}
Check "test"                     { Hx test $iso -q | Out-Null; if ($LASTEXITCODE -eq 0) { "all files readable" } else { "FAIL" } }
Check "split + read .001"        { Hx split $iso --size 1 -q | Out-Null; $j = Hx info "$iso.001" --json | ConvertFrom-Json; if ($j.files -ge 3) { "joined read ok" } else { "FAIL" } }
Check "floppy"                   { $f = Join-Path $work "f.img"; Hx floppy $f (Join-Path $src "readme.txt") -q | Out-Null; $j = Hx info $f --json | ConvertFrom-Json; if ($j.filesystem -eq "FAT" -and $j.files -eq 1) { "FAT12 ok" } else { "FAIL" } }
Check "exit codes"               { Hx bogus | Out-Null; $a = $LASTEXITCODE; Hx info (Join-Path $work "missing.iso") | Out-Null; $b = $LASTEXITCODE; if ($a -eq 2 -and $b -eq 1) { "usage=2, failure=1" } else { "FAIL $a $b" } }
Check "drives"                   { $d = Hx drives --json | ConvertFrom-Json; "$(@($d).Count) drive(s)" }
if ($Mount) {
    Check "mount / unmount"      {
        $l = (Hx mount $iso -q | Select-Object -First 1)
        $before = (Hx mounts --json | ConvertFrom-Json) | Where-Object { $_.image -like "*\t.iso" }
        Hx unmount $iso | Out-Null
        $after = (Hx mounts --json | ConvertFrom-Json) | Where-Object { $_.image -like "*\t.iso" }
        if ($l -match "^[A-Z]$" -and @($before).Count -ge 1 -and @($after).Count -eq 0) { "mounted as ${l}:, listed, ejected" } else { "FAIL '$l' before=$(@($before).Count) after=$(@($after).Count)" }
    }
}

# --- window: both renderers draw and save a picture ---
# In a disconnected remote-desktop session Direct3D can't present, so GPU checks are skipped there.
$disconnected = [bool]((query user 2>$null | Select-String '^>') -match '\sDisc\s')
foreach ($r in "cpu", "gpu") {
    if ($r -eq "gpu" -and $disconnected) { Write-Host ("[skip] {0,-34} {1}" -f "window / speed (gpu)", "desktop session disconnected: GPU frames can't be presented"); continue }
    Check "window ($r renderer)" {
        $env:HEXISO_RENDERER = $r
        $png = Join-Path $work "win-$r.png"
        $p = Start-Process -FilePath $Exe -ArgumentList "--screenshot", "`"$png`"" -PassThru
        if (-not $p.WaitForExit(90000)) { $p.Kill() }
        $env:HEXISO_RENDERER = $null
        if ((Test-Path $png) -and (Get-Item $png).Length -gt 20000) { "picture saved ({0} KB)" -f [int]((Get-Item $png).Length / 1KB) } else { "FAIL no picture" }
    }
}

# --- performance: frames per second, idle CPU ---
foreach ($r in "cpu", "gpu") {
    if ($r -eq "gpu" -and $disconnected) { continue }
    Check "redraw speed ($r)" {
        $env:HEXISO_RENDERER = $r; $env:HEXISO_BENCH = "1"; $env:HEXISO_FOREGROUND = "1"
        $log = Join-Path $work "bench-$r.txt"
        $p = Start-Process -FilePath $Exe -PassThru -NoNewWindow -RedirectStandardError $log
        if (-not $p.WaitForExit(60000)) { $p.Kill() }
        $env:HEXISO_RENDERER = $null; $env:HEXISO_BENCH = $null; $env:HEXISO_FOREGROUND = $null
        $line = Get-Content $log | Where-Object { $_ -match "^bench:" } | Select-Object -First 1
        if ($line -match "= ([\d.]+) fps") { if ([double]$matches[1] -ge 30 -or $r -eq "gpu") { $line.Substring(7) } else { "FAIL slow: $line" } } else { "FAIL no result" }
    }
}
Check "idle CPU" {
    $p = Start-Process -FilePath $Exe -PassThru
    Start-Sleep 4; $p.Refresh(); $c = $p.TotalProcessorTime.TotalSeconds; Start-Sleep 4; $p.Refresh()
    $d = $p.TotalProcessorTime.TotalSeconds - $c
    $null = $p.CloseMainWindow(); $p.WaitForExit(5000) | Out-Null
    if ($d -lt 0.2) { "{0:N3} CPU-seconds in 4 s" -f $d } else { "FAIL {0:N2} CPU-seconds in 4 s" -f $d }
}

Write-Host ""
if ($script:fails -eq 0) { Write-Host "SMOKE TEST PASSED ($work)"; exit 0 } else { Write-Host "SMOKE TEST: $script:fails FAILED ($work)"; exit 1 }
