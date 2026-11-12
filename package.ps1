# HEXISO release script. Builds the portable release:
#   dist\hexiso\hexiso.exe          <- the whole app (single file, no installer, no settings)
#   dist\hexiso\README.txt, LICENSE
#   dist\hexiso-<version>-win64.zip
#   dist\hexiso.json                <- Scoop manifest with the zip's SHA-256
# Usage:
#   powershell -ExecutionPolicy Bypass -File package.ps1 [-Url <zip download URL>] [-Homepage <project URL>]
#   (defaults: the v<version> release asset and https://github.com/Altair47/hexiso)
# Copyright (C) Nick Petropoulos. Licensed under AGPL-3.0-only.
param(
    [string]$Url = "",
    [string]$Homepage = "https://github.com/Altair47/hexiso"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$dist = Join-Path $PSScriptRoot "dist"
$folder = Join-Path $dist "hexiso"
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory -Force $folder | Out-Null
Copy-Item target\release\hexiso.exe $folder
Copy-Item LICENSE $folder
@"
HEXISO $version - portable disc image toolkit
Created by Nick Petropoulos - https://github.com/Altair47
Project page, source and updates: https://github.com/Altair47/hexiso
License: GNU AGPL-3.0 (see LICENSE)

Just run hexiso.exe. Nothing to install, no settings are saved and nothing is
written to the registry; temporary files (mount / burn copies) go to
%TEMP%\hexiso and can be deleted at any time.

Open an image directly:  hexiso.exe path\to\image.iso
Command line / scripting: hexiso help   (info, ls, extract, create, add, rm, convert,
  checksum, test, mount, unmount, burn, rip, usb ... with --json output and exit codes
  0 = ok, 1 = failed, 2 = wrong usage)
Requires Windows 10 or 11 (64-bit). Works without a graphics card (built-in CPU renderer).
"@ | Set-Content -Encoding utf8 (Join-Path $folder "README.txt")

$zipName = "hexiso-$version-win64.zip"
$zip = Join-Path $dist $zipName
Compress-Archive -Path (Join-Path $folder "*") -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
$utf8 = New-Object System.Text.UTF8Encoding($false)   # no byte-order mark: JSON / Scoop friendly
# checksum file published next to the zip (Scoop's autoupdate reads it)
[IO.File]::WriteAllText("$zip.sha256", "$hash  $zipName`n", $utf8)
# default: the GitHub release asset for this version (tag v<version>)
if ($Url -eq "") { $Url = "https://github.com/Altair47/hexiso/releases/download/v$version/$zipName" }

# Scoop manifest, ready for a bucket (ScoopInstaller/Extras or your own, e.g. github.com/Altair47/scoop):
# checkver + autoupdate let the bucket's update bot pick up new GitHub releases by itself.
$manifest = @"
{
    "version": "$version",
    "description": "Fast portable disc image toolkit: open, edit, extract, build, convert, compress, mount and burn ISO images.",
    "homepage": "$Homepage",
    "license": "AGPL-3.0-only",
    "architecture": {
        "64bit": {
            "url": "$Url",
            "hash": "$hash"
        }
    },
    "bin": "hexiso.exe",
    "shortcuts": [
        [
            "hexiso.exe",
            "HEXISO"
        ]
    ],
    "checkver": "github",
    "autoupdate": {
        "architecture": {
            "64bit": {
                "url": "https://github.com/Altair47/hexiso/releases/download/v`$version/hexiso-`$version-win64.zip",
                "hash": {
                    "url": "`$url.sha256"
                }
            }
        }
    }
}
"@
[IO.File]::WriteAllText((Join-Path $dist "hexiso.json"), $manifest.Replace("`r`n", "`n") + "`n", $utf8)
$null = $manifest | ConvertFrom-Json   # fails loudly if the JSON is ever broken

Write-Host "portable folder : $folder"
Write-Host "zip             : $zip"
Write-Host "sha256          : $hash  ($zipName.sha256)"
Write-Host "scoop manifest  : $(Join-Path $dist 'hexiso.json')"
