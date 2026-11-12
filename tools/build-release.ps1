# Build a release: tests, release binaries, third-party notices, installer.
#   powershell -ExecutionPolicy Bypass -File tools\build-release.ps1
# Output: dist\WinFX-<version>-setup.exe
# Needs: Rust (MSVC), cargo-about (cargo install cargo-about --features cli),
#        Inno Setup 6 (winget install JRSoftware.InnoSetup).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
Write-Host "WinFX $version"

cargo test --workspace --release --quiet
if ($LASTEXITCODE -ne 0) { throw 'tests failed' }
cargo build --workspace --release --quiet
if ($LASTEXITCODE -ne 0) { throw 'build failed' }

cargo about generate --workspace --fail -o THIRD-PARTY-NOTICES.html about.hbs
if ($LASTEXITCODE -ne 0) { throw 'cargo about failed' }

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found' }
& $iscc /Q "/DAppVersion=$version" "/DSourceDir=$root\target\release" installer\winfx.iss
if ($LASTEXITCODE -ne 0) { throw 'installer build failed' }
Get-Item "dist\WinFX-$version-setup.exe" | Select-Object FullName, Length
