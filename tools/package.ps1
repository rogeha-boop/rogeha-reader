<#
.SYNOPSIS
  Builds Rogeha Reader and packs it into an MSIX for the Microsoft Store or for a local test.

  The package version is <major>.<minor>.<build>.0: major and minor from `version` in Cargo.toml, build = the
  build number shown in About (the number of commits), so every package is numbered higher than the one
  before, as the Store requires. The Store does not take a version that starts with 0.

  makeappx.exe and signtool.exe come from the Windows SDK (C:\Program Files (x86)\Windows Kits\10\bin).

.EXAMPLE
  ./tools/package.ps1           # unsigned MSIX for Store upload (the Store signs it)
  ./tools/package.ps1 -Sign     # "Rogeha Reader (test)": signed with the test certificate, installs next to the Store app
#>
param(
    [switch]$Sign
)
$ErrorActionPreference = 'Stop'

$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$build = (git -C $root rev-list --count HEAD).Trim()
if ($LASTEXITCODE) { throw 'Could not count the commits for the build number (is git installed?)' }
$cargoVersion = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"(\d+)\.(\d+)\.').Matches[0]
$Version = "$($cargoVersion.Groups[1].Value).$($cargoVersion.Groups[2].Value).$build.0"
if ($Version -match '^0\.') { throw "The Store does not take version $Version (it must not start with 0): raise `version` in Cargo.toml." }
$dirty = git -C $root status --porcelain
if ($dirty) { Write-Warning "There are uncommitted changes: the package is not built from a commit." }

function Get-SdkTool([string]$name) {
    $tool = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter $name -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName | Select-Object -Last 1
    if (-not $tool) { throw "$name not found: install the Windows SDK" }
    return $tool.FullName
}

Write-Host "Building Rogeha Reader $Version..."
# Through cmd: cargo reports progress on stderr, which Windows PowerShell 5.1 would turn into an error.
Push-Location $root
try {
    cmd /c "cargo build --release 2>&1" | Out-Host
    if ($LASTEXITCODE) { throw 'cargo build failed' }
} finally { Pop-Location }

$out = Join-Path $root 'artifacts'
$layout = Join-Path $out ('layout' + $(if ($Sign) { '-test' } else { '' }))
if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
New-Item -ItemType Directory -Force (Join-Path $layout 'Assets') | Out-Null
# The reader and its assistant (a separate program the reader starts on request).
Copy-Item (Join-Path $root 'target\release\rogeha-reader.exe') $layout -Force
Copy-Item (Join-Path $root 'target\release\rogeha-agent.exe') $layout -Force
Copy-Item (Join-Path $root 'package\Assets\*.png') (Join-Path $layout 'Assets') -Force
Copy-Item (Join-Path $root 'LICENSE') $layout -Force

$manifest = (Get-Content (Join-Path $root 'package\AppxManifest.xml') -Raw).
    Replace('$VERSION$', $Version).Replace('$ARCH$', 'x64').
    Replace('$NAME$', $(if ($Sign) { 'RogehaStudio.RogehaReaderTest' } else { 'RogehaStudio.RogehaReader' })).
    Replace('$DISPLAYNAME$', $(if ($Sign) { 'Rogeha Reader (test)' } else { 'Rogeha Reader' }))
[System.IO.File]::WriteAllText((Join-Path $layout 'AppxManifest.xml'), $manifest, (New-Object System.Text.UTF8Encoding $false))

$msix = Join-Path $out ("RogehaReader_${Version}_x64" + $(if ($Sign) { '_test-signed' } else { '' }) + '.msix')
& (Get-SdkTool 'makeappx.exe') pack /d $layout /p $msix /o | Out-Null
if ($LASTEXITCODE) { throw 'makeappx failed' }

if ($Sign) {
    # Rogeha Capture's self-signed test certificate, issued to the Store publisher the manifest carries.
    $pfx = 'C:\Users\rogeh\StudioProjects\SnapTray\tools\.cache\dev-signing.pfx'
    $password = 'snaptray-dev'
    if (-not (Test-Path $pfx)) { throw "No test certificate at ${pfx}: run Rogeha Capture's (C# app's) tools\package.ps1 -Sign once" }
    $cer = Join-Path $out 'RogehaReader-test.cer'
    $certificate = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2($pfx, $password)
    [System.IO.File]::WriteAllBytes($cer, $certificate.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Cert))
    & (Get-SdkTool 'signtool.exe') sign /fd SHA256 /f $pfx /p $password $msix | Out-Null
    if ($LASTEXITCODE) { throw 'signtool failed' }
    Write-Host ''
    Write-Host 'Signed for local testing. If the certificate is not trusted yet, trust it once (admin PowerShell):'
    Write-Host "  Import-Certificate -FilePath '$cer' -CertStoreLocation Cert:\LocalMachine\TrustedPeople"
    Write-Host 'then double-click the package.'
}

Write-Host ''
'Package: {0} ({1:N0} KB)' -f $msix, ((Get-Item $msix).Length / 1KB)
'SHA-256: ' + (Get-FileHash $msix -Algorithm SHA256).Hash
